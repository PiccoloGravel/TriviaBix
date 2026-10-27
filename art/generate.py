#!/usr/bin/env python3
"""Makes TriviaBix's generated art: cutscene backdrops, stickers, looping clips and category cards.

    python art/generate.py --backend stub            # placeholder art, no GPU (for trying scenes out)
    python art/generate.py --backend gpu             # the real thing: FLUX.1-schnell, BiRefNet, Wan2.2
    python art/generate.py --plan                    # what would be made, without making it
    python art/generate.py --only sticker:trophy,clip:neon-stage --force

What to make is in art/catalog.toml (plus one card per category in content/categories.toml). Files go
to web/cutscenes/assets/gen/ next to a manifest.json that the TV reads to know which art exists.
Each file remembers a hash of its prompt, seed and model, so a re-run only makes what changed.
To run on a rented GPU, see render_on_vast.py.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import traceback
from dataclasses import dataclass, field
from pathlib import Path

from PIL import Image

from backends.models import NeedsGpu
from library import Library, default_root, style_dir

ROOT = Path(__file__).resolve().parent.parent
CATALOG = ROOT / "art" / "catalog.toml"
CATEGORIES = ROOT / "content" / "categories.toml"
THEMES = ROOT / "art" / "themes.toml"
OUT = ROOT / "web" / "cutscenes" / "assets" / "gen"
KINDS = ("backdrop", "sticker", "category", "clip")
RATINGS = ("PG", "PG-13", "MA", "NC-17")

# What each kind is generated at, what it's shipped at, and the most it may weigh. The TV loads all of
# it before a game, over Wi-Fi, so the budgets matter more than the last bit of sharpness.
SPEC = {
    "backdrop": {"gen": (1344, 768), "ship": (1280, 720), "max_bytes": 220_000, "dir": "bg", "ext": "webp"},
    "category": {"gen": (1344, 768), "ship": (960, 540), "max_bytes": 150_000, "dir": "category", "ext": "webp"},
    "sticker": {"gen": (1024, 1024), "ship": (512, 512), "max_bytes": 120_000, "dir": "stickers", "ext": "webp"},
    "clip": {"gen": (1280, 704), "ship": (960, 540), "max_bytes": 1_500_000, "dir": "clips", "ext": "mp4",
             "frames": 97, "fps": 24},
}


def slugify(text: str) -> str:
    """Same as slug() in web/cutscenes.js, so a category's name finds its card."""
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


@dataclass
class Job:
    kind: str
    id: str
    prompt: str
    seed: int
    source: str | None = None  # clips: the backdrop id they animate
    aliases: list[str] = field(default_factory=list)  # categories: other slugs that should find this card
    tags: list[str] = field(default_factory=list)  # what scenes ask for: a cue, a feeling, a place
    rating: str = "PG"  # PG, PG-13, MA or NC-17; a room only sees art at or below its rating
    version: str = "v1"  # the catalog's art direction ([style] version), or the theme's
    body: str = ""  # the entry's own prompt, before the theme's style goes around it
    custom: bool = False  # added on the admin Art page, not in catalog.toml
    pack: str = ""  # the catalog [[pack]] it was made for, like a mini game's set

    @property
    def key(self) -> str:
        return f"{self.kind}:{self.id}"

    @property
    def path(self) -> str:
        s = SPEC[self.kind]
        return f"{s['dir']}/{self.id}.{s['ext']}"


DEFAULT_THEME = "bix"
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,39}$")


def load_studio(path: Path | None) -> dict:
    """What the admin Art page saved (in appdata): extra themes, category styles, and new or changed
    catalog entries. Missing means nothing has been changed there yet."""
    if not path or not Path(path).exists():
        return {}
    try:
        studio = json.loads(Path(path).read_text())
    except ValueError as e:
        raise SystemExit(f"{path} isn't valid JSON: {e}")
    return studio if isinstance(studio, dict) else {}


def load_themes(catalog: Path = CATALOG, studio: dict | None = None, themes: Path = THEMES) -> dict[str, dict]:
    """Every theme by id: the catalog's own [style] as "bix", the ones in themes.toml, then the admin's."""
    style = tomllib.loads(catalog.read_text()).get("style", {})
    out = {DEFAULT_THEME: {"id": DEFAULT_THEME, "name": style.get("name", "Bix cartoon"),
                           "description": "The house style every show starts with.",
                           "prompt": style.get("prompt", "").strip(), "palette": "",
                           "version": style.get("version", "v1"), "builtin": True}}
    builtin = tomllib.loads(themes.read_text()).get("theme", []) if themes.exists() else []
    for t in [*builtin, *((studio or {}).get("themes") or [])]:
        tid = t.get("id", "")
        if not ID_RE.match(tid):
            raise SystemExit(f"theme id {tid!r} should be lower-case letters, digits and dashes")
        if tid in out and out[tid].get("builtin") and t not in builtin:
            raise SystemExit(f"theme {tid!r} is built in; copy it under another id to change it")
        out[tid] = {"version": "v1", "palette": "", **t, "builtin": t in builtin}
    return out


def theme_out(root: Path, theme: str) -> Path:
    """Where a theme's TV files go: the default theme in the art folder itself (as before themes), the
    others each in their own themes/<id> beside it."""
    return Path(root) if theme == DEFAULT_THEME else Path(root) / "themes" / theme


def load_packs(catalog: Path = CATALOG) -> list[dict]:
    """The catalog's [[pack]]s, in order: sets of pieces made for one part of the show."""
    packs = tomllib.loads(catalog.read_text()).get("pack", [])
    for p in packs:
        if not ID_RE.match(str(p.get("id", ""))):
            raise SystemExit(f"pack {p.get('id')!r}: ids are lower-case letters, digits and dashes")
    return packs


def load_jobs(catalog: Path = CATALOG, categories: Path = CATEGORIES, studio: dict | None = None,
              theme: str = DEFAULT_THEME, themes: Path = THEMES) -> list[Job]:
    cat = tomllib.loads(catalog.read_text())
    studio = studio or {}
    all_themes = load_themes(catalog, studio, themes)
    if theme not in all_themes:
        raise SystemExit(f"there is no art theme called {theme!r}")
    style = dict(cat.get("style", {}))
    tmpl = dict(cat.get("category", {}))
    t = all_themes[theme]
    if theme != DEFAULT_THEME:
        # A theme swaps the look and keeps what makes cut-outs and title space work.
        style["prompt"] = ", ".join(p.strip() for p in (t.get("prompt", ""), t.get("palette", "")) if p and p.strip())
        if t.get("sticker"):
            style["sticker"] = ", ".join(p for p in (style.get("sticker", ""), t["sticker"]) if p)
        if t.get("backdrop"):
            style["backdrop"] = t["backdrop"]
        if t.get("category"):
            tmpl["prompt"] = t["category"]
        style["version"] = f"{theme}-{t.get('version', 'v1')}"
    base = style.get("prompt", "").strip()
    cat_styles = {**(studio.get("category_styles") or {}), **(t.get("category_styles") or {})}

    def full(prompt: str, extra: str = "") -> str:
        return ", ".join(p.strip() for p in (base, prompt, extra) if p and p.strip())

    # The raw entries, by kind:id, with the admin's changes on top: everywhere, then for this theme.
    raw: dict[str, dict] = {}
    for kind in ("backdrop", "sticker", "clip"):
        for e in cat.get(kind, []):
            if f"{kind}:{e['id']}" in raw:
                raise SystemExit(f"{kind}:{e['id']} is in the catalog twice")
            raw[f"{kind}:{e['id']}"] = {"kind": kind, **e}
    if tmpl and categories.exists():
        for c in tomllib.loads(categories.read_text()).get("category", []):
            body = tmpl["prompt"].format(name=c["name"], description=c.get("description", ""), emoji=c.get("emoji", ""))
            if cat_styles.get(c["slug"]):
                body = f"{body}, {cat_styles[c['slug']].strip()}"
            raw[f"category:{c['slug']}"] = {"kind": "category", "id": c["slug"], "prompt": body,
                                           "seed": tmpl.get("seed", 0), "rating": c.get("rating", "PG"),
                                           "tags": ["category", f"category:{c['slug']}"],
                                           "aliases": sorted({slugify(c["name"])} - {c["slug"]})}
    for scope in (None, theme):
        for e in studio.get("entries") or []:
            if e.get("theme") != scope:
                continue
            key = f"{e.get('kind')}:{e.get('id')}"
            if e.get("kind") not in KINDS or not ID_RE.match(str(e.get("id", ""))):
                raise SystemExit(f"{key}: entries need a kind ({', '.join(KINDS)}) and an id of letters, digits and dashes")
            if e.get("hidden"):
                raw.pop(key, None)
                continue
            if key not in raw:
                if e["kind"] == "category":
                    continue  # the category is gone from the game; its old style is just ignored
                if not e.get("prompt"):
                    raise SystemExit(f"{key}: a new piece needs a prompt")
                raw[key] = {"kind": e["kind"], "id": e["id"], "custom": True}
            raw[key].update({k: e[k] for k in ("prompt", "seed", "tags", "rating", "from") if e.get(k) is not None})
            if e["kind"] == "category":
                # Its own tags are how scenes find it; the admin's are added to them.
                raw[key]["tags"] = list(dict.fromkeys(["category", f"category:{e['id']}", *raw[key]["tags"]]))

    pack_ids = {p["id"] for p in load_packs(catalog)}

    def meta(entry: dict) -> dict:
        rating = entry.get("rating", "PG")
        if rating not in RATINGS:
            raise SystemExit(f"{entry.get('id')}: rating must be one of {', '.join(RATINGS)}")
        if entry.get("pack") and entry["pack"] not in pack_ids:
            raise SystemExit(f"{entry.get('id')}: there is no [[pack]] called {entry['pack']!r}")
        return {"tags": list(entry.get("tags", [])), "rating": rating, "version": style.get("version", "v1"),
                "body": entry["prompt"], "custom": bool(entry.get("custom")), "pack": entry.get("pack", "")}

    jobs: list[Job] = []
    backdrops = {}
    for key, e in raw.items():
        if e["kind"] == "backdrop":
            job = Job("backdrop", e["id"], full(e["prompt"], style.get("backdrop", "")), e.get("seed", 0), **meta(e))
            backdrops[e["id"]] = job
            jobs.append(job)
    for key, e in raw.items():
        if e["kind"] == "sticker":
            jobs.append(Job("sticker", e["id"], full(e["prompt"], style.get("sticker", "")), e.get("seed", 0), **meta(e)))
    for key, e in raw.items():
        if e["kind"] == "category":
            jobs.append(Job("category", e["id"], full(e["prompt"], style.get("backdrop", "")), e.get("seed", 0),
                            aliases=e["aliases"], **meta(e)))
    for key, e in raw.items():
        if e["kind"] != "clip":
            continue
        if e.get("from") not in backdrops:
            raise SystemExit(f"clip {e['id']}: there is no backdrop called {e.get('from')!r}")
        # The motion prompt alone; the look comes from the still it animates.
        m = meta(e)
        if "rating" not in e:
            m["rating"] = backdrops[e["from"]].rating
        jobs.append(Job("clip", e["id"], e["prompt"], e.get("seed", 0), source=e["from"], **m))
    return jobs


def job_hash(job: Job, backend_id: str, jobs_by_key: dict[str, Job]) -> str:
    parts = {"kind": job.kind, "prompt": job.prompt, "seed": job.seed, "backend": backend_id, "spec": SPEC[job.kind],
             "version": job.version}
    if job.source:
        src = jobs_by_key[f"backdrop:{job.source}"]
        parts["source"] = job_hash(src, backend_id, jobs_by_key)
    return hashlib.sha256(json.dumps(parts, sort_keys=True).encode()).hexdigest()[:16]


# ---------- Manifest ----------

def read_manifest(out: Path) -> dict:
    try:
        return json.loads((out / "manifest.json").read_text())
    except (OSError, ValueError):
        return {"files": {}}


def write_manifest(out: Path, manifest: dict) -> None:
    files = manifest.setdefault("files", {})
    # Category cards by slug, and by the slug of their name (the TV only sees the name).
    manifest["categories"] = {}
    for path, f in sorted(files.items()):
        if f.get("kind") == "category":
            for slug in [f["id"], *f.get("aliases", [])]:
                manifest["categories"][slug] = path
    manifest["_readme"] = ("Made by art/generate.py; the TV reads this to know which generated art exists. "
                           "Don't edit by hand: change art/catalog.toml and run it again.")
    out.mkdir(parents=True, exist_ok=True)
    tmp = out / "manifest.json.tmp"
    tmp.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    tmp.replace(out / "manifest.json")


def plan(jobs: list[Job], out: Path, backend_id: str, force: bool = False, only: set[str] | None = None) -> list[Job]:
    """The jobs whose file is missing or was made from a different prompt, seed or model."""
    by_key = {j.key: j for j in jobs}
    files = read_manifest(out).get("files", {})
    todo = []
    for j in jobs:
        if only is not None and j.key not in only and j.id not in only and f"pack:{j.pack}" not in only:
            continue
        have = files.get(j.path)
        fresh = have and have.get("hash") == job_hash(j, backend_id, by_key) and (out / j.path).exists()
        if force or not fresh:
            todo.append(j)
    return todo


# ---------- Encoding ----------

def fit(img: Image.Image, size: tuple[int, int]) -> Image.Image:
    """Scales and centre-crops to exactly `size`."""
    w, h = size
    scale = max(w / img.width, h / img.height)
    img = img.resize((round(img.width * scale), round(img.height * scale)), Image.LANCZOS)
    left, top = (img.width - w) // 2, (img.height - h) // 2
    return img.crop((left, top, left + w, top + h))


def trim(img: Image.Image, pad: int = 8) -> Image.Image:
    """Crops a cut-out to what's visible, so stickers size by their content, not empty margins."""
    box = img.getchannel("A").point(lambda a: 255 if a > 8 else 0).getbbox()
    if not box:
        return img
    l, t, r, b = box
    return img.crop((max(0, l - pad), max(0, t - pad), min(img.width, r + pad), min(img.height, b + pad)))


def encode_webp(img: Image.Image, max_bytes: int) -> bytes:
    """Highest quality that fits the budget."""
    data = b""
    for q in (84, 78, 72, 64, 56, 48, 40):
        buf = io.BytesIO()
        img.save(buf, "WEBP", quality=q, method=6, exact=img.mode == "RGBA")
        data = buf.getvalue()
        if len(data) <= max_bytes:
            return data
    print(f"  warning: still {len(data):,} bytes at the lowest quality", file=sys.stderr)
    return data


def encode_clip(frames: list[Image.Image], fps: int, size: tuple[int, int], max_bytes: int) -> bytes:
    """H.264 MP4 that plays forwards then backwards, so the loop has no seam. No audio track."""
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg is needed to make clips")
    with tempfile.TemporaryDirectory() as tmp:
        for i, f in enumerate(frames):
            fit(f.convert("RGB"), size).save(f"{tmp}/{i:04d}.png")
        data = b""
        for crf in (28, 31, 34, 37, 40):
            out = f"{tmp}/clip.mp4"
            subprocess.run(
                ["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(fps), "-i", f"{tmp}/%04d.png",
                 "-filter_complex", "[0]split[a][b];[b]reverse[r];[a][r]concat=n=2:v=1[v]", "-map", "[v]",
                 "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-pix_fmt", "yuv420p",
                 "-profile:v", "high", "-movflags", "+faststart", "-an", out],
                check=True,
            )
            data = Path(out).read_bytes()
            if len(data) <= max_bytes:
                return data
        print(f"  warning: clip is still {len(data):,} bytes", file=sys.stderr)
        return data


def frames_to_mp4(frames: list[Image.Image], fps: int) -> bytes:
    """The clip at full size and near-lossless, played once (no ping-pong), for the asset store."""
    with tempfile.TemporaryDirectory() as tmp:
        for i, f in enumerate(frames):
            f.convert("RGB").save(f"{tmp}/{i:04d}.png")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(fps), "-i", f"{tmp}/%04d.png",
                        "-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p", "-an", f"{tmp}/o.mp4"], check=True)
        return Path(f"{tmp}/o.mp4").read_bytes()


def mp4_to_frames(path: Path) -> list[Image.Image]:
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(path), f"{tmp}/%04d.png"], check=True)
        frames = []
        for f in sorted(Path(tmp).glob("*.png")):
            img = Image.open(f)
            img.load()
            frames.append(img)
        return frames


# ---------- Running ----------

def get_backend(name: str):
    if name == "stub":
        from backends.stub import StubBackend
        return StubBackend()
    if name == "gpu":
        from backends.gpu import GpuBackend
        return GpuBackend()
    if name == "offline":
        from backends.models import OfflineBackend
        return OfflineBackend()
    raise SystemExit(f"unknown backend {name!r} (stub, gpu or offline)")


class Maker:
    """Turns jobs into shipped files, going through the asset store so every original is kept and
    nothing is rendered twice: a picture or clip asked for again comes from the store, not the GPU."""

    def __init__(self, backend, library: Library, art_version: str, jobs_by_key: dict[str, Job]):
        self.backend, self.lib, self.version, self.by_key = backend, library, art_version, jobs_by_key

    def style(self, kind: str) -> str:
        return f"{self.backend.style_name(kind)}:{self.version}"

    def meta(self, job: Job) -> dict:
        # The entry's own prompt and seed too, so the admin Art page can offer an earlier take back.
        return {"rating": job.rating, "tags": [job.kind, *job.tags], "catalog": job.key, "body": job.body,
                "seed": job.seed}

    def picture_inputs(self, job: Job, size: tuple[int, int]) -> dict:
        return {"prompt": job.prompt, "seed": job.seed, "width": size[0], "height": size[1],
                "steps": self.backend.image_steps}

    def picture(self, job: Job, size: tuple[int, int]) -> tuple[Image.Image, Path]:
        return self.lib.image(self.style("image"), self.picture_inputs(job, size),
                              lambda: self.backend.image(job.prompt, job.seed, *size),
                              model=self.backend.models("image")[0], **self.meta(job))

    def prefetch(self, todo: list[Job], batch: int = 0) -> None:
        """Makes everything `todo` needs from each model in one go, before the jobs run: every picture the
        store doesn't have (in batches of `batch`, same-sized ones together), then every sticker's cut-out.
        Each model is then loaded once, instead of swapping the picture and cut-out models for every
        sticker, and the GPU draws several pictures at a time. Clips come last, in `run`. A batch that
        fails is left for `make`, which tries those one at a time."""
        batch = batch or getattr(self.backend, "batch", 1)
        wanted: dict[tuple, dict] = {}
        for job in todo:
            if job.kind in ("backdrop", "category", "sticker"):
                pic = (job, SPEC[job.kind]["gen"])
            elif job.kind == "clip":
                pic = (self.by_key[f"backdrop:{job.source}"], SPEC["backdrop"]["gen"])
            else:
                continue
            inputs = self.picture_inputs(*pic)
            if not self.lib.find("image", self.style("image"), inputs):
                wanted.setdefault((pic[1], json.dumps(inputs, sort_keys=True)), pic)
        style, model = self.style("image"), self.backend.models("image")[0]
        pics = sorted(wanted.values(), key=lambda p: p[1])
        if pics:
            print(f"Drawing {len(pics)} pictures, {batch} at a time", flush=True)
        done = 0
        for size in sorted({p[1] for p in pics}):
            group = [p[0] for p in pics if p[1] == size]
            for i in range(0, len(group), batch):
                chunk = group[i:i + batch]
                t = time.time()
                try:
                    images = self.draw(chunk, size)
                except NeedsGpu:
                    return
                except Exception as e:  # noqa: BLE001 (make() retries these one at a time)
                    if broken_gpu(e):
                        raise SystemExit(f"Stopping: the GPU failed in a way every job would ({e!r})")
                    print(f"  a batch of {len(chunk)} failed ({e!r}); they'll be tried one at a time", file=sys.stderr)
                    continue
                for job, img in zip(chunk, images):
                    self.lib.put_image(style, self.picture_inputs(job, size), img, model=model, **self.meta(job))
                done += len(chunk)
                print(f"  [{done}/{len(pics)}] {len(chunk)} pictures in {time.time() - t:.1f}s", flush=True)
        stickers = [j for j in todo if j.kind == "sticker"]
        cut_style, cut_model = self.style("cutout"), self.backend.models("cutout")[0]
        for job in stickers:
            inputs = self.picture_inputs(job, SPEC["sticker"]["gen"])
            raw_path = self.lib.find("image", style, inputs)
            if not raw_path or self.lib.find("image", cut_style, {"source": raw_path.stem, "op": "cutout"}):
                continue
            try:
                raw = Image.open(raw_path)
                raw.load()
                cut = self.backend.cutout(raw)
            except NeedsGpu:
                return
            except Exception as e:  # noqa: BLE001
                if broken_gpu(e):
                    raise SystemExit(f"Stopping: the GPU failed in a way every job would ({e!r})")
                print(f"  cut-out for {job.key} failed ({e!r}); it'll be tried again", file=sys.stderr)
                continue
            self.lib.put_image(cut_style, {"source": raw_path.stem, "op": "cutout"}, cut, model=cut_model,
                               **self.meta(job))

    def draw(self, jobs: list[Job], size: tuple[int, int]) -> list[Image.Image]:
        """Pictures for `jobs`, in one call to the model if the backend can batch."""
        if hasattr(self.backend, "images"):
            return self.backend.images([j.prompt for j in jobs], [j.seed for j in jobs], *size)
        return [self.backend.image(j.prompt, j.seed, *size) for j in jobs]

    def make(self, job: Job) -> tuple[bytes, Path]:
        """The small file the TV gets, and the stored original it came from."""
        spec = SPEC[job.kind]
        if job.kind in ("backdrop", "category"):
            img, orig = self.picture(job, spec["gen"])
            return encode_webp(fit(img.convert("RGB"), spec["ship"]), spec["max_bytes"]), orig
        if job.kind == "sticker":
            raw, raw_path = self.picture(job, spec["gen"])
            inputs = {"source": raw_path.stem, "op": "cutout"}
            cut, orig = self.lib.image(self.style("cutout"), inputs, lambda: self.backend.cutout(raw),
                                       model=self.backend.models("cutout")[0], **self.meta(job))
            img = trim(cut.convert("RGBA"))
            img.thumbnail(spec["ship"], Image.LANCZOS)
            return encode_webp(img, spec["max_bytes"]), orig
        if job.kind == "clip":
            src = self.by_key[f"backdrop:{job.source}"]
            still, still_path = self.picture(src, SPEC["backdrop"]["gen"])
            still = fit(still.convert("RGB"), spec["gen"])
            inputs = {"source": still_path.stem, "prompt": job.prompt, "seed": job.seed, "width": spec["gen"][0],
                      "height": spec["gen"][1], "frames": spec["frames"], "fps": spec["fps"],
                      "steps": self.backend.video_steps}
            style = self.style("video")
            orig = self.lib.find("clip", style, inputs)
            if orig:
                self.lib.hits += 1
                frames = mp4_to_frames(orig)
            else:
                frames = self.backend.video(still, job.prompt, job.seed, spec["frames"], spec["fps"])
                orig = self.lib.put("clip", style, inputs, frames_to_mp4(frames, spec["fps"]), ".mp4",
                                    model=self.backend.models("video")[0], content_type="video/mp4",
                                    **self.meta(job), source_image=still_path.name)
            return encode_clip(frames, spec["fps"], spec["ship"], spec["max_bytes"]), orig
        raise ValueError(job.kind)


def broken_gpu(e: Exception) -> bool:
    """A CUDA error (such as "no kernel image is available") leaves the GPU unusable for this run, so going
    on would only fail every remaining job while the rental bills."""
    return "CUDA error" in str(e)


def run(jobs: list[Job], todo: list[Job], backend, out: Path, library: Library, art_version: str = "v1") -> int:
    by_key = {j.key: j for j in jobs}
    maker = Maker(backend, library, art_version, by_key)
    if hasattr(backend, "fetch"):
        # Every model the work needs comes down first, with its speed watched (a slow host is given back).
        kinds = {j.kind for j in todo}
        needs = {"image"} if kinds - {"clip"} else set()
        if "sticker" in kinds:
            needs.add("cutout")
        if "clip" in kinds:
            needs.add("video")
        backend.fetch(needs)
    maker.prefetch(todo)
    manifest = read_manifest(out)
    manifest.setdefault("files", {})
    failed = 0
    pending = []
    for n, job in enumerate(todo, 1):
        print(f"[{n}/{len(todo)}] {job.key}", flush=True)
        t = time.time()
        try:
            data, orig = maker.make(job)
        except NeedsGpu:
            pending.append(job.key)
            print("  not in the asset store yet; needs a GPU run")
            continue
        except Exception as e:  # one bad prompt shouldn't throw away a paid-for GPU hour
            if broken_gpu(e):
                raise SystemExit(f"Stopping: the GPU failed in a way every job would ({e!r})")
            failed += 1
            print(f"  failed: {e!r}", file=sys.stderr)
            if failed == 1:  # the first in full, so the log shows where it came from
                traceback.print_exc()
            continue
        dest = out / job.path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(data)
        entry = {"kind": job.kind, "id": job.id, "tags": job.tags, "rating": job.rating,
                 "prompt": job.prompt, "seed": job.seed, "backend": backend.id,
                 "hash": job_hash(job, backend.id, by_key), "bytes": len(data),
                 "original": str(orig.relative_to(library.root))}
        if job.aliases:
            entry["aliases"] = job.aliases
        if job.source:
            entry["from"] = SPEC["backdrop"]["dir"] + f"/{job.source}.webp"
        manifest["files"][job.path] = entry
        write_manifest(out, manifest)  # after every file, so a run that dies keeps what it made
        print(f"  {len(data):,} bytes in {time.time() - t:.1f}s", flush=True)
    print(f"Asset store {library.root}: {library.made} new originals, {library.hits} reused.")
    if pending:
        print(f"{len(pending)} need a GPU: {','.join(pending)}")
        out.mkdir(parents=True, exist_ok=True)
        (out / ".needs-gpu").write_text("\n".join(pending) + "\n")
    else:
        (out / ".needs-gpu").unlink(missing_ok=True)
    return failed


def retag(jobs: list[Job], out: Path) -> int:
    """Copies tags and ratings from the catalog onto art that's already made. They don't change the
    picture, so changing them never costs a GPU run."""
    manifest = read_manifest(out)
    files = manifest.get("files", {})
    changed = 0
    for j in jobs:
        f = files.get(j.path)
        if not f:
            continue
        want = {"tags": j.tags, "rating": j.rating, "aliases": j.aliases}
        if any(f.get(k, [] if k != "rating" else None) != v for k, v in want.items()):
            f.update(want)
            if not j.aliases:
                f.pop("aliases", None)
            changed += 1
    if changed:
        write_manifest(out, manifest)
    return changed


def prune(jobs: list[Job], out: Path) -> list[str]:
    """Drops files the catalog no longer lists."""
    keep = {j.path for j in jobs}
    manifest = read_manifest(out)
    gone = [p for p in manifest.get("files", {}) if p not in keep]
    for p in gone:
        (out / p).unlink(missing_ok=True)
        del manifest["files"][p]
    if gone:
        write_manifest(out, manifest)
    return gone


def merge(src: Path, out: Path) -> int:
    """Copies a run's files (say, downloaded from a rented GPU) into `out` and merges the manifests."""
    theirs = read_manifest(src).get("files", {})
    if not theirs:
        raise SystemExit(f"no manifest.json with files in {src}")
    manifest = read_manifest(out)
    manifest.setdefault("files", {})
    for path, entry in theirs.items():
        if not (src / path).exists():
            continue
        (out / path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src / path, out / path)
        manifest["files"][path] = entry
    write_manifest(out, manifest)
    return len(theirs)


def status_of(jobs: list[Job], out: Path, backend_id: str = "gpu") -> dict[str, str]:
    """kind:id -> made (up to date), stale (made from an older prompt or seed) or missing."""
    by_key = {j.key: j for j in jobs}
    files = read_manifest(out).get("files", {})
    out_of = {}
    for j in jobs:
        have = files.get(j.path)
        if not have or not (out / j.path).exists():
            out_of[j.key] = "missing"
        else:
            out_of[j.key] = "made" if have.get("hash") == job_hash(j, backend_id, by_key) else "stale"
    return out_of


def takes(jobs: list[Job], library: Library, backend) -> dict[str, list[dict]]:
    """kind:id -> the pictures the asset store has for it in this theme, newest first: every earlier
    prompt and seed it was drawn with, so one can be put back for free."""
    maker = Maker(backend, library, jobs[0].version if jobs else "v1", {})
    folder = library.root / "image" / style_dir(maker.style("image"))
    out: dict[str, list[dict]] = {}
    for meta_path in sorted(folder.glob("*.json")) if folder.is_dir() else []:
        try:
            meta = json.loads(meta_path.read_text())
        except (OSError, ValueError):
            continue
        key, inputs = meta.get("catalog"), meta.get("inputs") or {}
        if not key or key.startswith("clip:") or not (folder / meta.get("file", "")).exists():
            continue
        out.setdefault(key, []).append({
            "seed": inputs.get("seed"), "body": meta.get("body"), "prompt": inputs.get("prompt"),
            "original": f"image/{folder.name}/{meta['file']}", "created": meta.get("created", 0)})
    for v in out.values():
        v.sort(key=lambda t: -t["created"])
    return out


def listing(catalog: Path, categories: Path, studio: dict, theme: str, root: Path, library: Library,
            backend_id: str = "gpu") -> dict:
    """Everything the admin Art page shows, as JSON: the themes with how much of each is made, and
    for `theme` every piece with its prompt, tags, state and stored takes."""
    themes = load_themes(catalog, studio)
    out = []
    for t in themes.values():
        jobs = load_jobs(catalog, categories, studio, t["id"])
        st = status_of(jobs, theme_out(root, t["id"]), backend_id)
        out.append({**t, "made": sum(v == "made" for v in st.values()), "total": len(jobs),
                    "folder": str(theme_out(Path(""), t["id"]))})
    jobs = load_jobs(catalog, categories, studio, theme)
    by_key = {j.key: j for j in jobs}
    st = status_of(jobs, theme_out(root, theme), backend_id)
    if backend_id == "stub":
        from backends.stub import StubBackend as Looker
    else:
        from backends.models import OfflineBackend as Looker
    stored = takes(jobs, library, Looker())
    files = read_manifest(theme_out(root, theme)).get("files", {})
    items = []
    for j in jobs:
        prefix = "" if theme == DEFAULT_THEME else f"themes/{theme}/"
        pic = by_key[f"backdrop:{j.source}"] if j.source else j
        items.append({"key": j.key, "kind": j.kind, "id": j.id, "body": j.body, "prompt": j.prompt, "seed": j.seed,
                      "tags": j.tags, "rating": j.rating, "from": j.source, "custom": j.custom, "status": st[j.key],
                      "pack": j.pack,
                      "file": prefix + j.path if j.path in files else None,
                      "takes": [] if j.source else stored.get(pic.key, [])})
    cats = tomllib.loads(categories.read_text()).get("category", []) if categories.exists() else []
    t = themes[theme]
    cat_styles = {**(studio.get("category_styles") or {}), **(t.get("category_styles") or {})}
    packs = [{**p, "made": sum(st[j.key] == "made" for j in jobs if j.pack == p["id"]),
              "total": sum(j.pack == p["id"] for j in jobs)} for p in load_packs(catalog)]
    return {"theme": theme, "themes": out, "items": items, "packs": packs,
            "categories": [{"slug": c["slug"], "name": c["name"], "emoji": c.get("emoji", ""),
                            "style": cat_styles.get(c["slug"], ""), "theme_style": bool((t.get("category_styles") or {}).get(c["slug"]))}
                           for c in cats],
            "tags": sorted({tag for j in jobs for tag in j.tags}),
            "hidden": sorted({f"{e.get('kind')}:{e.get('id')}" for e in studio.get("entries") or []
                              if e.get("hidden") and e.get("theme") in (None, theme)})}


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--backend", default="stub",
                    help="stub (placeholders, no GPU), gpu (the real models), or offline (only what the asset store has)")
    ap.add_argument("--out", type=Path, default=OUT, help="where the art and manifest.json go")
    ap.add_argument("--library", type=Path, default=default_root(),
                    help="the asset store where full-size originals are kept and reused (default $ASSETS_DIR, "
                         "else art/library); nothing in it is shipped to the TV")
    ap.add_argument("--catalog", type=Path, default=CATALOG)
    ap.add_argument("--categories", type=Path, default=CATEGORIES)
    ap.add_argument("--only", help="comma-separated ids, kind:id or pack:id (e.g. trophy,clip:neon-stage,pack:quick-draw)")
    ap.add_argument("--kinds", help=f"comma-separated subset of {','.join(KINDS)}")
    ap.add_argument("--force", action="store_true", help="make them even if nothing changed")
    ap.add_argument("--plan", action="store_true", help="list what would be made, then stop")
    ap.add_argument("--prune", action="store_true", help="delete art the catalog no longer lists")
    ap.add_argument("--merge", type=Path, metavar="DIR", help="merge another run's output into --out, then stop")
    ap.add_argument("--studio", type=Path, default=os.environ.get("ART_STUDIO") or None,
                    help="the admin Art page's themes, category styles and entries (JSON, default $ART_STUDIO)")
    ap.add_argument("--theme", help=f"the art theme to make (default: the one in use, else {DEFAULT_THEME}); "
                                    "--out should be that theme's folder")
    ap.add_argument("--list", type=Path, metavar="ART_DIR",
                    help="print the admin Art page's JSON for --theme, with ART_DIR as the art root, then stop")
    args = ap.parse_args(argv)

    if args.merge:
        print(f"merged {merge(args.merge, args.out)} files into {args.out}")
        return 0
    studio = load_studio(args.studio)
    theme = args.theme or studio.get("active") or DEFAULT_THEME
    if args.list:
        print(json.dumps(listing(args.catalog, args.categories, studio, theme, args.list, Library(args.library),
                                 "stub" if args.backend == "stub" else "gpu")))
        return 0
    jobs = load_jobs(args.catalog, args.categories, studio, theme)
    if not args.plan and retag(jobs, args.out):
        print("updated tags and ratings in manifest.json")
    if args.prune:
        for p in prune(jobs, args.out):
            print(f"removed {p}")
    if args.kinds:
        kinds = set(args.kinds.split(","))
        if kinds - set(KINDS):
            raise SystemExit(f"unknown kinds: {', '.join(sorted(kinds - set(KINDS)))}")
        selected = [j for j in jobs if j.kind in kinds]
    else:
        selected = jobs
    only = set(args.only.split(",")) if args.only else None
    backend_id = "stub" if args.backend == "stub" else "gpu"  # offline makes the same files gpu would
    todo = plan(selected, args.out, backend_id, args.force, only)
    if args.plan:
        for j in todo:
            print(j.key)
        print(f"{len(todo)} to make, {len(selected) - len(todo)} up to date", file=sys.stderr)
        return 0
    if not todo:
        (args.out / ".needs-gpu").unlink(missing_ok=True)
        print("Everything is up to date.")
        return 0
    backend = get_backend(args.backend)
    failed = run(jobs, todo, backend, args.out, Library(args.library), jobs[0].version if jobs else "v1")
    if args.backend == "offline" and (args.out / ".needs-gpu").exists():
        return 3  # not a failure: render_on_vast.py reads .needs-gpu
    print(f"Made {len(todo) - failed} of {len(todo)}." + (f" {failed} failed." if failed else ""))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    sys.exit(main())
