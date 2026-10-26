"""Placeholder art drawn with Pillow: no GPU, no downloads, same output every time.

It lets the whole pipeline (sizes, budgets, cut-outs, clips, manifest, the TV picking art up) be
tried and tested without paying for a GPU. Each picture is a sunburst in colours picked from the
seed and prompt, with the item's name on it, so it's obvious which file landed where.
"""

from __future__ import annotations

import hashlib
import math
import random

from PIL import Image, ImageDraw, ImageFont

PALETTE = ["#e63973", "#2f80ed", "#f2a900", "#7b5cff", "#27ae60", "#ff6b35", "#00b8a9", "#1a0b3d"]


def _font(size: int):
    for name in ("DejaVuSans-Bold.ttf", "Arial Bold.ttf", "LiberationSans-Bold.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default(size)


def _label(prompt: str) -> str:
    """The item's own words (after the shared style text), cut to a few words."""
    text = prompt.split("no text, ", 1)[-1].split(":")[0].split(",")[0]
    return " ".join(text.split()[:4]).upper()


class StubBackend:
    id = "stub"
    image_steps = 0
    video_steps = 0

    def models(self, kind: str) -> list[str]:
        return ["stub"]

    def style_name(self, kind: str) -> str:
        return f"stub-{kind}"

    def _rng(self, prompt: str, seed: int) -> random.Random:
        return random.Random(int(hashlib.sha256(f"{seed}:{prompt}".encode()).hexdigest()[:12], 16))

    def image(self, prompt: str, seed: int, width: int, height: int) -> Image.Image:
        rng = self._rng(prompt, seed)
        c0, c1 = rng.sample(PALETTE[:-1], 2)
        img = Image.new("RGB", (width, height), c0)
        d = ImageDraw.Draw(img)
        cx, cy, r = width / 2, height / 2, math.hypot(width, height)
        for i in range(0, 360, 20):
            a0, a1 = math.radians(i), math.radians(i + 10)
            d.polygon([(cx, cy), (cx + r * math.cos(a0), cy + r * math.sin(a0)), (cx + r * math.cos(a1), cy + r * math.sin(a1))], fill=c1)
        for _ in range(14):
            x, y, s = rng.uniform(0, width), rng.uniform(0, height), rng.uniform(0.02, 0.07) * height
            d.ellipse((x - s, y - s, x + s, y + s), fill="#fff8f0", outline="#1a0b3d", width=max(2, int(s / 6)))
        font = _font(max(18, height // 14))
        d.text((cx, cy), _label(prompt), font=font, fill="#fff8f0", anchor="mm", stroke_width=max(2, height // 160), stroke_fill="#1a0b3d")
        return img

    def cutout(self, img: Image.Image) -> Image.Image:
        """A round badge cut out of the picture, on a clear background."""
        w, h = img.size
        mask = Image.new("L", (w, h), 0)
        m = min(w, h) * 0.12
        ImageDraw.Draw(mask).ellipse((m, m, w - m, h - m), fill=255)
        out = img.convert("RGBA")
        out.putalpha(mask)
        ImageDraw.Draw(out).ellipse((m, m, w - m, h - m), outline="#fff8f0", width=int(min(w, h) * 0.03))
        return out

    def video(self, still: Image.Image, prompt: str, seed: int, frames: int, fps: int) -> list[Image.Image]:
        """A slow push-in on the still, with a light sweeping across."""
        w, h = still.size
        out = []
        for i in range(frames):
            t = i / max(1, frames - 1)
            z = 1 + 0.08 * t
            cw, ch = w / z, h / z
            f = still.crop(((w - cw) / 2, (h - ch) / 2, (w + cw) / 2, (h + ch) / 2)).resize((w, h))
            glow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
            x = w * t
            ImageDraw.Draw(glow).ellipse((x - h / 3, h / 3, x + h / 3, h), fill=(255, 248, 240, 60))
            out.append(Image.alpha_composite(f.convert("RGBA"), glow).convert("RGB"))
        return out
