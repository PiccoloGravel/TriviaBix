"""Checks batch.py's batched sampler against Ming's own one-line sampler, step for step, on a tiny randomly
initialised copy of the model, so it runs on a CPU in seconds. Needs torch and Ming's source:

    MING_DIR=/opt/ming python3 -m unittest tts/test_ming_batch.py    (skipped without them)
"""

import os
import sys
import types
import unittest
from pathlib import Path

MING = os.environ.get("MING_DIR", "/opt/ming")
try:
    import torch
    HAVE = Path(MING, "modeling_bailingmm.py").exists()
except ImportError:
    HAVE = False


def load_ming():
    sys.path.insert(0, MING)
    # The audio codec is never built here; only its import has to succeed.
    for name in ("diffusers", "diffusers.models", "diffusers.models.autoencoders",
                 "diffusers.models.autoencoders.autoencoder_oobleck"):
        mod = sys.modules.setdefault(name, types.ModuleType(name))
        mod.OobleckDiagonalGaussianDistribution = object
    import modeling_bailingmm
    return modeling_bailingmm


@unittest.skipUnless(HAVE, "needs torch and Ming's source in MING_DIR")
class BatchedSamplerTest(unittest.TestCase):
    def tiny(self):
        mb = load_ming()
        from fm.dit import Aggregator
        from fm.flowloss import FlowLoss
        from transformers import Qwen2Config, Qwen2ForCausalLM

        torch.manual_seed(0)
        m = types.SimpleNamespace()
        m.model = Qwen2ForCausalLM(Qwen2Config(vocab_size=64, hidden_size=32, intermediate_size=64,
                                               num_hidden_layers=2, num_attention_heads=4, num_key_value_heads=2,
                                               attn_implementation="sdpa")).eval()
        m.latent_dim, m.patch_size, m.history_patch_size = 8, 2, 4
        m.flowloss = FlowLoss(z_channels=8, llm_cond_dim=32, hidden_size=32, depth=2, num_heads=4).eval()
        m.linear_proj_audio = Aggregator(in_channels=8, hidden_size=32, depth=1, num_heads=4, llm_input_dim=32).eval()
        # Weights picked so the three rows below stop at different steps (6, 7 and 9).
        m.stop_head = torch.nn.Linear(32, 2)
        with torch.no_grad():
            m.stop_head.weight.copy_(torch.randn(2, 32, generator=torch.Generator().manual_seed(32)))
            m.stop_head.bias.copy_(torch.tensor([0.0, 1.0]))
        m.model_type, m.device = "dense", torch.device("cpu")
        prompts = {}

        def prepare_input_embed(prompt, text, **_):
            return torch.zeros(prompts[text].shape[:2], dtype=torch.long), prompts[text]
        m.tokenizer = types.SimpleNamespace(decode=lambda ids: "")
        m.prepare_input_embed = prepare_input_embed
        m.sample = types.MethodType(mb.BailingMMNativeForConditionalGeneration.sample, m)
        return m, prompts

    def test_batched_rows_match_one_at_a_time(self):
        import batch
        m, prompts = self.tiny()
        gen = torch.Generator().manual_seed(1)
        for n, text in enumerate(["short", "a bit longer", "the longest of the three lines"]):
            prompts[text] = torch.randn(1, 5 + 3 * n, 32, generator=gen)
        # Starting noise isn't drawn the same way one line at a time and in a batch; with it at zero (and
        # temperature 0, as Ming runs) both are deterministic and must agree.
        real_randn = torch.randn
        torch.randn = lambda *a, **k: torch.zeros(*a, **{x: v for x, v in k.items() if x in ("dtype", "device")})
        try:
            with torch.inference_mode():
                one = [torch.cat([lat for lat, _ in m.sample(prompt=None, text=t, max_decode_steps=12)], dim=1)
                       for t in prompts]
                mb = batch.MingBatch.__new__(batch.MingBatch)
                mb.torch, mb.max_cache = torch, 512
                mb.ming = types.SimpleNamespace(model=m)
                many = mb.sample([{"prompt": None, "text": t, "prompt_text": None, "max_decode_steps": 12}
                                  for t in prompts])
        finally:
            torch.randn = real_randn
        self.assertGreater(len({a.shape[1] for a in one}), 1, "the rows should stop at different steps")
        for a, b in zip(one, many):
            self.assertEqual(a.shape, b.shape)
            torch.testing.assert_close(a, b, rtol=1e-4, atol=1e-4)


class FakeMing:
    """MingTTS.synthesize's shape: splits a line into fragments, asks the model's `generate` for each, and
    joins what comes back."""

    sample_rate = 100

    def __init__(self):
        self.model = types.SimpleNamespace(device=torch.device("cpu"), generate=self.generate)
        self.model.audio = types.SimpleNamespace(decode=lambda lat, **_: (lat.reshape(1, 1, -1).float(),))
        del self.model.generate  # MingBatch sets and removes it on the instance

    def generate(self, **kw):  # the class attribute a real model has
        return torch.full((1, len(kw["text"])), float(len(kw["text"])))

    def synthesize(self, task, text, **_):
        if "boom" in text:
            raise ValueError("can't say boom")
        gen = getattr(self.model, "generate", self.generate)
        return torch.cat([gen(prompt=task, text=frag) for frag in text.split("|")], dim=-1)


@unittest.skipUnless(HAVE, "needs torch")
class SynthesizeTest(unittest.TestCase):
    def test_lines_come_back_whole_and_in_order(self):
        import batch
        mb = batch.MingBatch.__new__(batch.MingBatch)
        mb.torch, mb.ming, mb.chained = torch, FakeMing(), False
        mb.voice = types.SimpleNamespace(kwargs=lambda text: {"task": "IP", "text": text})
        # Each fragment's "latents" are its length repeated, so a line's audio shows which pieces made it.
        mb.sample = lambda rows: [torch.full((1, len(kw["text"])), float(len(kw["text"]))) for kw in rows]
        out = mb.synthesize(["ab|cde", "boom", "fghi"])
        self.assertEqual(out[0].tolist(), [2.0] * 2 + [3.0] * 3)
        self.assertIsInstance(out[1], ValueError)
        self.assertEqual(out[2].tolist(), [4.0] * 4)
        self.assertFalse(hasattr(mb.ming.model, "generate"), "the model is left as it was")


if __name__ == "__main__":
    unittest.main()
