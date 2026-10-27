#!/usr/bin/env python3
"""Makes the art that needs a GPU on a rented vast.ai GPU, then brings it home.

    VAST_API_KEY=... python3 art/render_on_vast.py --dry-run   # what it would make, which GPU, the price
    VAST_API_KEY=... python3 art/render_on_vast.py             # do it (asks before renting)

1. Anything the asset store already has is turned into the TV's files here, for free.
2. Only what's left goes to the GPU: gpu/vast.py rents the cheapest card within --max-price, runs the
   art image (art/Dockerfile) on it, copies the results back and always destroys the instance.
3. The new originals go into the asset store (ASSETS_DIR) and the TV's files into
   web/cutscenes/assets/gen/, ready to commit.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import shlex
import signal
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from backends.models import CUTOUT_MODEL, IMAGE_MODEL, VIDEO_MODEL

ART = Path(__file__).resolve().parent
ROOT = ART.parent
VAST = ROOT / "gpu" / "vast.py"
IMAGE = os.environ.get("ART_IMAGE", "ghcr.io/piccologravel/triviabix-art:latest")

# Rough timings on an RTX 4090, for the time limit and the estimate: model downloads at boot, then
# FLUX.1-schnell pictures a few seconds each (drawn in batches), and Wan2.2 clips in minutes. A card with
# less than OFFLOAD_GB has to move the models in and out, which gpu/vast.py counts in when picking one.
BOOT_MIN, PER_PICTURE_MIN, PER_CLIP_MIN = 5, 0.15, 7
# How long a GPU gets to pull the art image and start; one slower than that is given back for the next.
BOOT_LIMIT_MIN = 10
# What a run downloads, in GB, before it starts drawing: the image (unless the host has it) and each model
# the work needs. Counted at each host's speed when picking one, so a fast connection wins.
IMAGE_GB, PICTURE_MODEL_GB, CUTOUT_MODEL_GB, CLIP_MODEL_GB = 10, 34, 1, 24
# The slowest host connection art rents, in Mbit/s; the estimate prefers faster ones.
MIN_INET = 500
OFFLOAD_GB = 40
# Minutes each theme after the first adds for loading the models again (from the GPU's disk).
RELOAD_MIN = 2


def model_access(repos: list[str], token: str | None) -> str | None:
    """Why the GPU couldn't download one of `repos` from Hugging Face, if it couldn't: a gated model (FLUX.1)
    needs a token from an account that accepted its terms. Checked before renting, so a run doesn't pay for
    a GPU only to fail at the first download. If Hugging Face can't be reached from here, the run goes ahead."""
    hub = os.environ.get("HF_ENDPOINT", "https://huggingface.co").rstrip("/")
    for repo in dict.fromkeys(repos):
        try:
            with urllib.request.urlopen(f"{hub}/api/models/{repo}", timeout=20) as res:
                info = json.load(res)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return f"the art model {repo} doesn't exist on Hugging Face; check the model on the Settings page"
            continue
        except (urllib.error.URLError, OSError, ValueError) as e:
            print(f"Couldn't check {repo} on Hugging Face ({e}); going ahead.")
            continue
        if not info.get("gated"):
            continue
        terms = f"{hub}/{repo}"
        if not token:
            return (f"{repo} is a gated model: accept its terms at {terms} while logged in to Hugging Face, then "
                    "add a Hugging Face token (read access) on the admin Settings page")
        files = [s["rfilename"] for s in info.get("siblings") or [] if not s["rfilename"].startswith(".")
                 and s["rfilename"] != "README.md"]
        probe = "model_index.json" if "model_index.json" in files else (files or ["config.json"])[0]
        req = urllib.request.Request(f"{hub}/{repo}/resolve/main/{urllib.parse.quote(probe)}", method="HEAD",
                                     headers={"Authorization": f"Bearer {token}"})
        try:
            urllib.request.urlopen(req, timeout=20).close()
        except urllib.error.HTTPError as e:
            if e.code in (401, 403):
                return (f"the Hugging Face token on the Settings page can't download {repo} ({e.code}). Accept the "
                        f"model's terms at {terms} with the account the token belongs to, or add a token from that "
                        "account (read access)")
        except (urllib.error.URLError, OSError) as e:
            print(f"Couldn't check {repo} on Hugging Face ({e}); going ahead.")
    return None


def ship_code(dest: Path, studio: Path | None = None):
    """Copies the generator's code and what it reads (the catalog, the themes, the categories, and the
    admin Art page's changes as art/studio.json) into `dest`, laid out as in the image's /app."""
    for rel in ("art/generate.py", "art/library.py", "art/catalog.toml", "art/themes.toml", "content/categories.toml",
                *(str(p.relative_to(ROOT)) for p in sorted((ART / "backends").glob("*.py")))):
        (dest / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / rel, dest / rel)
    if studio and studio.exists():
        shutil.copy2(studio, dest / "art" / "studio.json")


def event(name: str, **data):
    """A line the game server's admin panel reads (TBX_EVENTS=1)."""
    if os.environ.get("TBX_EVENTS"):
        print("@tbx " + json.dumps({"event": name, **data}), file=sys.stderr, flush=True)


def generate(*args: str) -> int:
    return subprocess.call([sys.executable, str(ART / "generate.py"), *args])


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=ROOT / "web" / "cutscenes" / "assets" / "gen")
    ap.add_argument("--library", type=Path, default=Path(os.environ.get("ASSETS_DIR") or ART / "library"),
                    help="the asset store (default $ASSETS_DIR, else art/library)")
    ap.add_argument("--only", help="limit to these ids or kind:id, as in generate.py")
    ap.add_argument("--kinds", help="limit to these kinds, as in generate.py")
    ap.add_argument("--force", action="store_true", help="remake the TV files even if nothing changed")
    ap.add_argument("--image", default=IMAGE, help=f"the art image to run (default {IMAGE})")
    ap.add_argument("--min-vram", default="24", help="GB of GPU memory (default 24)")
    ap.add_argument("--max-price", default="0.60", help="$/hour, all in (default 0.60)")
    ap.add_argument("--disk", default="80", help="GB of disk; the models are about 45 GB (default 80)")
    ap.add_argument("--yes", action="store_true", help="rent without asking")
    ap.add_argument("--dry-run", action="store_true", help="show what would be made and the price, rent nothing")
    ap.add_argument("--studio", type=Path, default=os.environ.get("ART_STUDIO") or None,
                    help="the admin Art page's themes and entries, as in generate.py")
    ap.add_argument("--theme", default="bix",
                    help="the art theme to make; several separated by commas, or all, make them all on one GPU "
                         "(each into its folder under --art-root)")
    ap.add_argument("--art-root", type=Path,
                    help="the art folder the themes' folders are in (as generate.py's theme_out); needed for "
                         "several themes, and used instead of --out when given")
    args = ap.parse_args(argv)

    if args.studio and args.studio.exists():
        # The admin page's changes as they were when the run started, for every step of it, even if
        # someone edits a piece while it runs.
        snap = Path(tempfile.mkdtemp(prefix="triviabix-studio-")) / "studio.json"
        shutil.copy2(args.studio, snap)
        args.studio = snap
    if not VAST.exists():
        raise SystemExit(f"{VAST} is missing: it comes with the vast.ai runner (gpu/README.md).")
    themes = pick_themes(args.theme, args.studio)
    if len(themes) > 1 and not args.art_root:
        raise SystemExit("--art-root is needed to make several themes")
    outs = {t: theme_out(args.art_root, t) if args.art_root else args.out for t in themes}

    # 1. Everything the store already has, made locally, theme by theme.
    picked = [a for flag, v in (("--only", args.only), ("--kinds", args.kinds)) if v for a in (flag, v)]
    if args.force:
        picked.append("--force")
    studio = ["--studio", str(args.studio)] if args.studio else []

    def common(theme: str) -> list[str]:
        return ["--out", str(outs[theme]), "--library", str(args.library), "--theme", theme, *studio, *picked]

    todos: dict[str, list[str]] = {}
    for theme in themes:
        code = generate("--backend", "offline", *common(theme))
        needs = outs[theme] / ".needs-gpu"
        if code != 0 and needs.exists():
            todos[theme] = needs.read_text().split()
        elif code != 0:
            return code
    if not todos:
        print("Nothing needs a GPU.")
        event("todo", count=0, clips=0)
        return 0
    todo = [k for keys in todos.values() for k in keys]
    clips = sum(k.startswith("clip:") for k in todo)
    stickers = sum(k.startswith("sticker:") for k in todo)
    work = (len(todo) - clips) * PER_PICTURE_MIN + clips * PER_CLIP_MIN
    download = (IMAGE_GB + PICTURE_MODEL_GB * (len(todo) > clips) + CUTOUT_MODEL_GB * (stickers > 0)
                + CLIP_MODEL_GB * (clips > 0))
    # The download at the slowest connection rented, as gpu/vast.py counts it.
    fetch = download * 8000 / (MIN_INET * 0.5) / 60
    # Each theme after the first loads the models again, from the GPU's own disk this time.
    reload = RELOAD_MIN * (len(todos) - 1)
    minutes = BOOT_MIN + fetch / 2 + work + reload
    # Room for a card half as fast as a 4090 on the slowest connection allowed.
    limit = max(30, int(BOOT_MIN + fetch + (work + reload) * 2))
    event("todo", count=len(todo), clips=clips, minutes=round(minutes), limit=limit, themes=len(todos))
    if len(todos) > 1:
        print("\n" + "\n".join(f"{t}: {len(keys)} to make" for t, keys in todos.items()))
    print(f"\n{len(todo)} need a GPU ({clips} clips){f' across {len(todos)} themes, on one GPU' if len(todos) > 1 else ''}. "
          f"About {minutes:.0f} minutes; time limit {limit} minutes.")
    if problem := model_access([IMAGE_MODEL, CUTOUT_MODEL, VIDEO_MODEL], os.environ.get("HF_TOKEN")):
        print(f"Not renting a GPU: {problem}.", file=sys.stderr)
        return 1

    # 2. The rest on one rented GPU, theme after theme. The container writes each theme's TV files to
    # /out/gen/<theme> and the originals to /out/assets. It runs this server's copy of the generator
    # (uploaded to /in/code), so a fix takes effect without rebuilding the image; the image supplies
    # PyTorch and the models' libraries. Faster model downloads where the image allows: hf_xet's
    # high-performance mode, and hf_transfer for repos not on xet (installed if missing; skipped if that fails).
    fast = ("export HF_XET_HIGH_PERFORMANCE=1; if python3 -c 'import hf_transfer' 2>/dev/null || "
            "pip install -q hf_transfer >/dev/null 2>&1; then export HF_HUB_ENABLE_HF_TRANSFER=1; fi; ")
    cmd = f"{fast}cp -r /in/code/. /app/ && cd /app || exit 1; rc=0; " + gpu_steps(todos, bool(args.studio and args.studio.exists())) + "exit $rc"
    with tempfile.TemporaryDirectory(prefix="triviabix-art-") as tmp, \
            tempfile.TemporaryDirectory(prefix="triviabix-art-code-") as code:
        ship_code(Path(code) / "code", args.studio)
        run = [sys.executable, str(VAST), "run", "--image", args.image, "--cmd", cmd, "--out", "/out", "--to", tmp,
               "--input", code, "--keep-partial",
               "--min-vram", args.min_vram, "--max-price", args.max_price, "--disk", args.disk,
               "--max-minutes", str(limit), "--work-minutes", f"{work + reload:.1f}", "--boot-estimate", str(BOOT_MIN),
               "--offload-below", str(OFFLOAD_GB), "--kind", "art", "--download-gb", str(download),
               "--boot-minutes", str(BOOT_LIMIT_MIN)]
        # The token and the models picked on the admin panel's Settings page go to the GPU too.
        for key in ("HF_TOKEN", "ART_IMAGE_MODEL", "ART_CUTOUT_MODEL", "ART_VIDEO_MODEL"):
            if os.environ.get(key):
                run += ["--env", f"{key}={os.environ[key]}"]
        run += ["--yes"] * args.yes + ["--dry-run"] * args.dry_run
        # The art image's PyTorch (2.7, CUDA 12.8) runs Blackwell cards too, but needs a host driver for 12.8.
        env = {"VAST_MAX_COMPUTE_CAP": "1200", "VAST_MIN_CUDA": "12.8", "VAST_MIN_INET": str(MIN_INET), **os.environ}
        # Cancelling (SIGTERM to the whole process group) stops gpu/vast.py, which gives the GPU back and
        # returns; what came home by then is still saved below.
        signal.signal(signal.SIGTERM, lambda *_: None)
        code = subprocess.Popen(run, env=env).wait()
        if args.dry_run:
            return code

        # 3. Bring home whatever it made, even if some items failed.
        got = Path(tmp)
        if (got / "assets").is_dir():
            shutil.copytree(got / "assets", args.library, dirs_exist_ok=True)
            print(f"Saved the originals in {args.library}")
        for theme in todos:
            if (got / "gen" / theme / "manifest.json").exists():
                generate("--merge", str(got / "gen" / theme), "--out", str(outs[theme]))
                # Turn the new originals into TV files here too, which also clears .needs-gpu.
                generate("--backend", "offline", *common(theme))
        return code


def pick_themes(spec: str, studio: Path | None) -> list[str]:
    """The themes `spec` names (ids separated by commas, or all), each checked to exist."""
    from generate import load_studio, load_themes

    known = load_themes(studio=load_studio(studio))
    if spec.strip() == "all":
        return list(known)
    themes = list(dict.fromkeys(t.strip() for t in spec.split(",") if t.strip()))
    if not themes:
        raise SystemExit("no art theme given")
    for t in themes:
        if t not in known:
            raise SystemExit(f"there is no art theme called {t!r}")
    return themes


def theme_out(root: Path, theme: str) -> Path:
    from generate import theme_out as out

    return out(root, theme)


def gpu_steps(todos: dict[str, list[str]], studio: bool) -> str:
    """The shell steps that make each theme's pieces on the GPU, one theme after another. A theme that fails
    doesn't stop the next, and the run ends with its exit code; a host found too slow to download (75) stops
    at once, so gpu/vast.py can rent another."""
    steps = ""
    for theme, keys in todos.items():
        steps += (f"python3 art/generate.py --backend gpu --out /out/gen/{shlex.quote(theme)} --library /out/assets "
                  f"--only {shlex.quote(','.join(keys))} --force --theme {shlex.quote(theme)}"
                  + (" --studio art/studio.json" if studio else "")
                  + "; c=$?; if [ $c -eq 75 ]; then exit 75; fi; if [ $c -ne 0 ]; then rc=$c; fi; ")
    return steps


if __name__ == "__main__":
    sys.exit(main())
