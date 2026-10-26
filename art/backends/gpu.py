"""The real generators. Needs an NVIDIA GPU with 24 GB or more and the packages in requirements.txt.

- Pictures: FLUX.1-schnell (Apache-2.0), 4 steps, so a picture takes a few seconds. Drawn several at a
  time (ART_BATCH, default 8, or 4 on a 24 GB card).
- Cut-outs: BiRefNet (MIT) takes the background off stickers.
- Clips: Wan2.2-TI2V-5B (Apache-2.0) animates a backdrop still.

Each model loads the first time it's needed and is let go before the next one loads, so one 24 GB
card can run all three. Weights come from Hugging Face (about 45 GB the first time); point HF_HOME
at a disk that keeps them between runs.
"""

from __future__ import annotations

import gc
import os

import numpy as np
import torch
from PIL import Image

from .models import CUTOUT_MODEL, IMAGE_MODEL, VIDEO_MODEL, VIDEO_STEPS, GpuModels

# Wan's usual negative prompt, cut down to what matters for cartoon backdrops.
VIDEO_NEGATIVE = ("blurry, low quality, jpeg artifacts, distorted, deformed, flicker, text, subtitles, watermark, "
                  "static frame, overexposed, messy background, extra limbs, fast camera motion")


class GpuBackend(GpuModels):

    def __init__(self):
        if not torch.cuda.is_available():
            raise SystemExit("No CUDA GPU here. Use --backend stub, or run on a GPU box (see render_on_vast.py).")
        major, minor = torch.cuda.get_device_capability()
        arches = torch.cuda.get_arch_list()
        cap = major * 10 + minor
        if f"sm_{cap}" not in arches and not any(a.startswith("compute_") and int(a.split("_")[1]) <= cap for a in arches):
            # Every model would fail with "no kernel image is available"; stop before paying for downloads.
            raise SystemExit(f"This PyTorch build ({torch.__version__}) has no kernels for {torch.cuda.get_device_name()} "
                             f"(sm_{cap}); it was built for {', '.join(arches)}.")
        self.device = "cuda"
        self.loaded: tuple[str, object] | None = None
        big = torch.cuda.get_device_properties(0).total_memory >= 40 * 2**30
        # On a 24 GB card the models don't fit whole, so parts move to the GPU only while they run.
        self.offload = os.environ.get("ART_OFFLOAD", "auto") == "on" or (os.environ.get("ART_OFFLOAD", "auto") == "auto" and not big)
        # Pictures drawn per call to FLUX. More at once keeps the GPU busy; a batch that runs out of memory
        # is split in half and tried again.
        self.batch = int(os.environ.get("ART_BATCH") or (4 if self.offload else 8))

    def _use(self, name: str, load):
        """Keeps one model in memory at a time."""
        if self.loaded and self.loaded[0] == name:
            return self.loaded[1]
        self.loaded = None
        gc.collect()
        torch.cuda.empty_cache()
        model = load()
        self.loaded = (name, model)
        return model

    def _pipe(self, pipe):
        if self.offload:
            pipe.enable_model_cpu_offload()
        else:
            pipe.to(self.device)
        return pipe

    def image(self, prompt: str, seed: int, width: int, height: int) -> Image.Image:
        from diffusers import FluxPipeline

        pipe = self._use("image", lambda: self._pipe(FluxPipeline.from_pretrained(IMAGE_MODEL, torch_dtype=torch.bfloat16)))
        return pipe(
            prompt,
            width=width,
            height=height,
            guidance_scale=0.0,  # schnell is distilled: no guidance, 4 steps
            num_inference_steps=4,
            max_sequence_length=256,
            generator=torch.Generator("cpu").manual_seed(seed),
        ).images[0]

    def images(self, prompts: list[str], seeds: list[int], width: int, height: int) -> list[Image.Image]:
        """Several pictures in one call, each from its own seed, so each comes out as `image` would make it
        alone (the asset store's keys stay the same)."""
        from diffusers import FluxPipeline

        pipe = self._use("image", lambda: self._pipe(FluxPipeline.from_pretrained(IMAGE_MODEL, torch_dtype=torch.bfloat16)))
        try:
            return pipe(
                list(prompts),
                width=width,
                height=height,
                guidance_scale=0.0,
                num_inference_steps=4,
                max_sequence_length=256,
                generator=[torch.Generator("cpu").manual_seed(s) for s in seeds],
            ).images
        except torch.cuda.OutOfMemoryError:
            if len(prompts) == 1:
                raise
            gc.collect()
            torch.cuda.empty_cache()
            half = len(prompts) // 2
            self.batch = max(1, half)
            return (self.images(prompts[:half], seeds[:half], width, height)
                    + self.images(prompts[half:], seeds[half:], width, height))

    def cutout(self, img: Image.Image) -> Image.Image:
        from torchvision import transforms
        from transformers import AutoModelForImageSegmentation

        def load():
            m = AutoModelForImageSegmentation.from_pretrained(CUTOUT_MODEL, trust_remote_code=True)
            return m.to(self.device).eval().half()

        model = self._use("cutout", load)
        prep = transforms.Compose([
            transforms.Resize((1024, 1024)),
            transforms.ToTensor(),
            transforms.Normalize([0.485, 0.456, 0.406], [0.229, 0.224, 0.225]),
        ])
        rgb = img.convert("RGB")
        with torch.no_grad():
            pred = model(prep(rgb).unsqueeze(0).to(self.device).half())[-1].sigmoid().cpu()[0].squeeze()
        mask = transforms.functional.to_pil_image(pred.float()).resize(rgb.size)
        out = rgb.convert("RGBA")
        out.putalpha(mask)
        return out

    def video(self, still: Image.Image, prompt: str, seed: int, frames: int, fps: int) -> list[Image.Image]:
        from diffusers import AutoencoderKLWan, WanImageToVideoPipeline

        def load():
            vae = AutoencoderKLWan.from_pretrained(VIDEO_MODEL, subfolder="vae", torch_dtype=torch.float32)
            return self._pipe(WanImageToVideoPipeline.from_pretrained(VIDEO_MODEL, vae=vae, torch_dtype=torch.bfloat16))

        pipe = self._use("video", load)
        result = pipe(
            image=still,
            prompt=prompt,
            negative_prompt=VIDEO_NEGATIVE,
            width=still.width,
            height=still.height,
            num_frames=frames,  # Wan wants 4n+1
            guidance_scale=5.0,
            num_inference_steps=VIDEO_STEPS,
            generator=torch.Generator("cpu").manual_seed(seed),
        ).frames[0]
        return [f if isinstance(f, Image.Image) else Image.fromarray((np.asarray(f) * 255).clip(0, 255).astype("uint8")) for f in result]
