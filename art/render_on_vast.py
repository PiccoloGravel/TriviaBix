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
import subprocess
import sys
import tempfile
from pathlib import Path

ART = Path(__file__).resolve().parent
ROOT = ART.parent
VAST = ROOT / "gpu" / "vast.py"
IMAGE = os.environ.get("ART_IMAGE", "ghcr.io/piccologravel/triviabix-art:latest")

# Rough timings on an RTX 4090, for the time limit and the estimate: model downloads at boot, then
# FLUX.1-schnell pictures in seconds and Wan2.2 clips in minutes.
BOOT_MIN, PER_PICTURE_MIN, PER_CLIP_MIN = 15, 0.25, 7


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
    args = ap.parse_args(argv)

    if not VAST.exists():
        raise SystemExit(f"{VAST} is missing: it comes with the vast.ai runner (gpu/README.md).")

    # 1. Everything the store already has, made locally.
    picked = [a for flag, v in (("--only", args.only), ("--kinds", args.kinds)) if v for a in (flag, v)]
    if args.force:
        picked.append("--force")
    common = ["--out", str(args.out), "--library", str(args.library), *picked]
    code = generate("--backend", "offline", *common)
    needs = args.out / ".needs-gpu"
    if code == 0 or not needs.exists():
        if code == 0:
            print("Nothing needs a GPU.")
            event("todo", count=0, clips=0)
        return code
    todo = needs.read_text().split()
    clips = sum(k.startswith("clip:") for k in todo)
    minutes = BOOT_MIN + (len(todo) - clips) * PER_PICTURE_MIN + clips * PER_CLIP_MIN
    limit = max(30, int(minutes * 1.5))
    event("todo", count=len(todo), clips=clips, minutes=round(minutes), limit=limit)
    print(f"\n{len(todo)} need a GPU ({clips} clips). About {minutes:.0f} minutes; time limit {limit} minutes.")

    # 2. The rest on a rented GPU. The container writes TV files to /out/gen and originals to /out/assets.
    cmd = (f"python3 art/generate.py --backend gpu --out /out/gen --library /out/assets "
           f"--only {','.join(todo)} --force")
    with tempfile.TemporaryDirectory(prefix="triviabix-art-") as tmp:
        run = [sys.executable, str(VAST), "run", "--image", args.image, "--cmd", cmd, "--out", "/out", "--to", tmp,
               "--min-vram", args.min_vram, "--max-price", args.max_price, "--disk", args.disk,
               "--max-minutes", str(limit)]
        if os.environ.get("HF_TOKEN"):
            run += ["--env", f"HF_TOKEN={os.environ['HF_TOKEN']}"]
        run += ["--yes"] * args.yes + ["--dry-run"] * args.dry_run
        code = subprocess.call(run)
        if args.dry_run:
            return code

        # 3. Bring home whatever it made, even if some items failed.
        got = Path(tmp)
        if (got / "assets").is_dir():
            shutil.copytree(got / "assets", args.library, dirs_exist_ok=True)
            print(f"Saved the originals in {args.library}")
        if (got / "gen" / "manifest.json").exists():
            generate("--merge", str(got / "gen"), "--out", str(args.out))
            # Turn the new originals into TV files here too, which also clears .needs-gpu.
            generate("--backend", "offline", *common)
        return code


if __name__ == "__main__":
    sys.exit(main())
