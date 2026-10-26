"""The shared store for everything TriviaBix generates (see docs/assets.md). Python 3.9+, stdlib only.

    from assets import Store
    store = Store("/data/assets")                     # or Store() for $ASSETS_DIR
    inputs = {"prompt": "Bix bowing", "width": 1024, "height": 576, "seed": 7}
    hit = store.find("image", "flux.1-schnell:bix-cartoon-v1", inputs)
    if hit is None:
        png = render(...)                             # only when it isn't there yet
        hit = store.put("image", "flux.1-schnell:bix-cartoon-v1", inputs, png, ".png",
                        model="black-forest-labs/FLUX.1-schnell", tags=["cutscene"], rating="PG")
    print(hit.path, hit.meta)

From the command line:

    python3 gpu/assets.py list [--kind image] [--tag cutscene] [--max-rating PG-13]
    python3 gpu/assets.py index            # writes assets/index.json, a listing rebuilt from the .json files
    python3 gpu/assets.py import DIR --kind image --style STYLE    # files with sidecar .json from a job
"""

from __future__ import annotations

import argparse
import hashlib
import json
import mimetypes
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

RATINGS = ["PG", "PG-13", "MA", "NC-17"]
DEFAULT_DIR = "/data/assets" if Path("/data").is_dir() else "data/assets"


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def key_of(inputs: dict) -> str:
    return hashlib.sha256(canonical(inputs).encode()).hexdigest()[:32]


def slug_of(style: str) -> str:
    return hashlib.sha256(style.encode()).hexdigest()[:12]


@dataclass
class Asset:
    path: Path
    meta: dict


class Store:
    def __init__(self, root: str | os.PathLike | None = None):
        self.root = Path(root or os.environ.get("ASSETS_DIR") or DEFAULT_DIR)

    def folder(self, kind: str, style: str) -> Path:
        return self.root / kind / slug_of(style)

    def find(self, kind: str, style: str, inputs: dict) -> Asset | None:
        """The asset made from exactly these inputs in this style, if it was ever made."""
        folder, key = self.folder(kind, style), key_of(inputs)
        if not folder.is_dir():
            return None
        for path in folder.glob(key + ".*"):
            if path.suffix in (".json", ".part"):
                continue
            return Asset(path, self._meta(path.with_suffix(".json")))
        return None

    def put(self, kind: str, style: str, inputs: dict, data: bytes, ext: str, **meta) -> Asset:
        """Saves an asset and its metadata. Extra keyword arguments (model, tags, rating, ...) go in the
        metadata as they are."""
        folder, key = self.folder(kind, style), key_of(inputs)
        folder.mkdir(parents=True, exist_ok=True)
        style_file = folder / "style.txt"
        if not style_file.exists():
            _write(style_file, style.encode())
        ext = ext if ext.startswith(".") else "." + ext
        path = folder / (key + ext)
        _write(path, data)
        record = {
            "kind": kind,
            "style": style,
            "inputs": inputs,
            "key": key,
            "file": path.name,
            "content_type": mimetypes.guess_type(path.name)[0] or "application/octet-stream",
            "bytes": len(data),
            "created": int(time.time()),
            **{k: v for k, v in meta.items() if v is not None},
        }
        if record.get("rating") not in (None, *RATINGS):
            raise ValueError(f"rating must be one of {RATINGS}")
        _write(path.with_suffix(".json"), json.dumps(record, indent=2, ensure_ascii=False).encode())
        return Asset(path, record)

    def all(self):
        """Every asset in the store, with whatever metadata it has."""
        for meta_path in sorted(self.root.glob("*/*/*.json")):
            meta = self._meta(meta_path)
            path = meta_path.with_name(meta.get("file", ""))
            if meta and path.is_file():
                yield Asset(path, meta)

    def index(self) -> Path:
        """Writes index.json: every asset's metadata with its path relative to the store. It's only a
        listing for convenience; the per-asset .json files are the truth, so rebuild it any time."""
        entries = [{**a.meta, "path": str(a.path.relative_to(self.root))} for a in self.all()]
        out = self.root / "index.json"
        _write(out, json.dumps(entries, indent=1, ensure_ascii=False).encode())
        return out

    @staticmethod
    def _meta(path: Path) -> dict:
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}


def _write(path: Path, data: bytes):
    """Write then rename, so a half-written file is never picked up."""
    tmp = path.with_name(path.name + ".part")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def allowed(meta: dict, max_rating: str | None) -> bool:
    if not max_rating:
        return True
    return RATINGS.index(meta.get("rating", "PG")) <= RATINGS.index(max_rating)


def main(argv=None):
    p = argparse.ArgumentParser(description="TriviaBix's store of generated assets (docs/assets.md)")
    p.add_argument("--dir", help="the store (default $ASSETS_DIR, else /data/assets)")
    sub = p.add_subparsers(dest="cmd", required=True)
    ls = sub.add_parser("list", help="list assets")
    ls.add_argument("--kind")
    ls.add_argument("--tag", action="append", default=[])
    ls.add_argument("--max-rating", choices=RATINGS)
    sub.add_parser("index", help="rebuild index.json")
    imp = sub.add_parser("import", help="add a job's output: each file with a <name>.json holding its inputs and metadata")
    imp.add_argument("src")
    imp.add_argument("--kind", required=True)
    imp.add_argument("--style", required=True)
    args = p.parse_args(argv)
    store = Store(args.dir)

    if args.cmd == "list":
        for a in store.all():
            m = a.meta
            if args.kind and m.get("kind") != args.kind:
                continue
            if not set(args.tag) <= set(m.get("tags", [])) or not allowed(m, args.max_rating):
                continue
            label = m.get("inputs", {}).get("text") or m.get("inputs", {}).get("prompt") or ""
            print(f"{a.path.relative_to(store.root)}  {m.get('rating', '-'):5}  {','.join(m.get('tags', []))}  {label[:70]}")
    elif args.cmd == "index":
        print(store.index())
    elif args.cmd == "import":
        added = skipped = 0
        for path in sorted(Path(args.src).rglob("*")):
            sidecar = path.with_name(path.name + ".json")
            if not path.is_file() or path.suffix == ".json" or not sidecar.exists():
                continue
            meta = json.loads(sidecar.read_text(encoding="utf-8"))
            inputs = meta.pop("inputs")
            for k in ("kind", "style", "key", "file", "content_type", "bytes"):
                meta.pop(k, None)
            if store.find(args.kind, args.style, inputs):
                skipped += 1
                continue
            store.put(args.kind, args.style, inputs, path.read_bytes(), path.suffix, **meta)
            added += 1
        print(f"added {added}, already there {skipped}")
        store.index()


if __name__ == "__main__":
    sys.exit(main())
