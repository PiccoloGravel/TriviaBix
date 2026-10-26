"""Tests art/backends/gpu.py's recovery paths with stand-in pipelines, on a CPU. Needs torch (skipped
without it):
    cd art && python3 -m unittest test_gpu_backend.py
"""

import os
import sys
import tempfile
import time
import unittest
import unittest.mock
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

try:
    import torch
    from backends import gpu
except ImportError:
    gpu = None


class Pipe:
    """A FluxPipeline stand-in that, like the real one with offload hooks, runs out of memory on more than
    `fits` prompts and is broken for good afterwards."""

    loads = 0

    def __init__(self, fits):
        Pipe.loads += 1
        self.fits, self.broken = fits, False

    def __call__(self, prompt, **kw):
        prompts = prompt if isinstance(prompt, list) else [prompt]
        if self.broken:
            raise RuntimeError("Expected all tensors to be on the same device, but found at least two devices")
        if len(prompts) > self.fits:
            self.broken = True
            raise torch.cuda.OutOfMemoryError("CUDA out of memory. Tried to allocate 74.00 MiB")
        return type("Out", (), {"images": [f"picture of {p}" for p in prompts]})()


@unittest.skipUnless(gpu, "needs torch")
class RecoveryTest(unittest.TestCase):
    def backend(self, fits):
        b = gpu.GpuBackend.__new__(gpu.GpuBackend)
        b.loaded, b.offload, b.batch = None, True, 4
        b._load_image = lambda: Pipe(fits)
        Pipe.loads = 0
        return b

    def test_a_batch_that_runs_out_of_memory_is_redrawn_on_a_fresh_pipeline(self):
        b = self.backend(fits=1)
        got = b.images(["a", "b", "c", "d"], [1, 2, 3, 4], 64, 64)
        self.assertEqual(got, [f"picture of {p}" for p in "abcd"])
        self.assertGreater(Pipe.loads, 1, "the broken pipeline was replaced")
        self.assertEqual(b.batch, 1)

    def test_one_picture_gets_one_fresh_try(self):
        b = self.backend(fits=1)
        b.loaded = ("image", Pipe(0))  # broken by an earlier call
        b.loaded[1].broken = False
        self.assertEqual(b.image("a", 1, 64, 64), "picture of a")

    def test_a_slow_download_gives_the_host_back(self):
        def trickle(repo, **kw):
            for n in range(100):
                Path(os.environ["HF_HOME"], f"part{n}").write_bytes(b"x" * 10)
                time.sleep(0.05)

        with tempfile.TemporaryDirectory() as home, \
                unittest.mock.patch.dict(os.environ, {"HF_HOME": home}), \
                unittest.mock.patch("huggingface_hub.constants.HF_HOME", home), \
                unittest.mock.patch("huggingface_hub.snapshot_download", trickle), \
                unittest.mock.patch.object(gpu, "SLOW_AFTER_S", 0.3), \
                unittest.mock.patch.object(gpu, "give_up", side_effect=SystemExit(gpu.SLOW_HOST)):
            b = gpu.GpuBackend.__new__(gpu.GpuBackend)
            with self.assertRaises(SystemExit) as stop:
                b.fetch({"cutout"})
        self.assertEqual(stop.exception.code, 75)


if __name__ == "__main__":
    unittest.main()
