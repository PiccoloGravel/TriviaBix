"""Which models the gpu backend runs. Kept apart from gpu.py so machines without torch (planning,
the asset-store-only `offline` backend) name things the same way."""

import os

IMAGE_MODEL = os.environ.get("ART_IMAGE_MODEL", "black-forest-labs/FLUX.1-schnell")
CUTOUT_MODEL = os.environ.get("ART_CUTOUT_MODEL", "ZhengPeng7/BiRefNet")
VIDEO_MODEL = os.environ.get("ART_VIDEO_MODEL", "Wan-AI/Wan2.2-TI2V-5B-Diffusers")
IMAGE_STEPS = 4  # FLUX.1-schnell is distilled for 4 steps
VIDEO_STEPS = int(os.environ.get("ART_VIDEO_STEPS", "30"))


class GpuModels:
    """Names and settings shared by the gpu and offline backends, so both find the same stored art."""

    id = "gpu"
    image_steps = IMAGE_STEPS
    video_steps = VIDEO_STEPS

    def models(self, kind: str) -> list[str]:
        """The model behind each step: image, cutout or video."""
        return [{"image": IMAGE_MODEL, "cutout": CUTOUT_MODEL, "video": VIDEO_MODEL}[kind]]

    def style_name(self, kind: str) -> str:
        """The asset store's style prefix: the model's short name, e.g. flux.1-schnell."""
        return self.models(kind)[0].split("/")[-1].lower().removesuffix("-diffusers")


class NeedsGpu(Exception):
    """Raised by the offline backend for anything the asset store doesn't already have."""


class OfflineBackend(GpuModels):
    """Makes the TV's files from originals already in the asset store, without a GPU. Whatever isn't
    stored yet is left for a GPU run (render_on_vast.py does this pass first, to rent less)."""

    def image(self, *a, **k):
        raise NeedsGpu()

    cutout = video = image
