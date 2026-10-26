"""Keeps every generated original so nothing is paid for twice, in the project's shared asset store.

The layout is the one the voice renderer uses too (docs/assets.md, gpu/assets.py):

    ASSETS_DIR/{kind}/{sha256(style)[:12]}/{key}.{ext}     the file
    ASSETS_DIR/{kind}/{sha256(style)[:12]}/{key}.json      what made it: inputs, model, tags, rating...
    ASSETS_DIR/{kind}/{sha256(style)[:12]}/style.txt       the style string, readable

`kind` is image or clip. `style` names what makes the same inputs come out differently (the model
and the art direction, e.g. "flux.1-schnell:bix-cartoon-v1"). `key` is a hash of the inputs (prompt,
seed, size, steps...), so asking for the same thing again finds the file instead of rendering it.
The per-file .json is the truth; `python3 gpu/assets.py index` rebuilds a listing from them.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import time
from pathlib import Path

from PIL import Image


def canonical(inputs: dict) -> str:
    return json.dumps(inputs, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def key_of(inputs: dict) -> str:
    return hashlib.sha256(canonical(inputs).encode()).hexdigest()[:32]


def style_dir(style: str) -> str:
    return hashlib.sha256(style.encode()).hexdigest()[:12]


class Library:
    def __init__(self, root: Path | str):
        self.root = Path(root)
        self.hits = 0
        self.made = 0

    def path(self, kind: str, style: str, inputs: dict, ext: str) -> Path:
        return self.root / kind / style_dir(style) / f"{key_of(inputs)}{ext}"

    def find(self, kind: str, style: str, inputs: dict) -> Path | None:
        """The stored file for these inputs, if it was ever made (with or without its .json)."""
        folder = self.root / kind / style_dir(style)
        if not folder.is_dir():
            return None
        for path in folder.glob(key_of(inputs) + ".*"):
            if path.suffix not in (".json", ".part"):
                return path
        return None

    def put(self, kind: str, style: str, inputs: dict, data: bytes, ext: str, *, model: str,
            content_type: str, rating: str = "PG", tags: list[str] | None = None, **extra) -> Path:
        dest = self.path(kind, style, inputs, ext)
        dest.parent.mkdir(parents=True, exist_ok=True)
        (dest.parent / "style.txt").write_text(style)
        part = dest.with_suffix(dest.suffix + ".part")  # only complete files ever have the real name
        part.write_bytes(data)
        part.replace(dest)
        meta = {"kind": kind, "style": style, "inputs": inputs, "key": key_of(inputs), "file": dest.name,
                "content_type": content_type, "bytes": len(data), "model": model, "created": int(time.time()),
                "generator": "art/generate.py", "rating": rating, "tags": list(tags or []), **extra}
        dest.with_suffix(".json").write_text(json.dumps(meta, indent=2, ensure_ascii=False) + "\n")
        self.made += 1
        return dest

    # Pictures, the common case.

    def image(self, style: str, inputs: dict, make, *, model: str, **meta) -> tuple[Image.Image, Path]:
        """The stored picture for `inputs`, or `make()`'s, stored on the way out."""
        hit = self.find("image", style, inputs)
        if hit:
            self.hits += 1
            img = Image.open(hit)
            img.load()
            return img, hit
        img = make()
        buf = io.BytesIO()
        img.save(buf, "PNG")
        return img, self.put("image", style, inputs, buf.getvalue(), ".png", model=model, content_type="image/png", **meta)


def default_root() -> Path:
    return Path(os.environ.get("ASSETS_DIR") or Path(__file__).resolve().parent / "library")
