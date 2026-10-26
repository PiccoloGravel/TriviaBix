"""Tests batch.py with the stub backend: python3 -m unittest tts/test_batch.py"""

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

os.environ["TTS_BACKEND"] = "stub"
import batch  # noqa: E402
import server  # noqa: E402


class BatchTest(unittest.TestCase):
    def test_lines_batching_cant_make_are_made_one_at_a_time_and_batching_stops(self):
        class Model:
            def synthesize(self, text):
                if "bad" in text:
                    raise ValueError("a bad line")
                return f"one:{text}"

        class Broken:  # the batched sampler fails every line, as it did with mis-shaped pieces
            def synthesize(self, texts):
                return [IndexError("tuple index out of range") for _ in texts]

        waves, many, said = batch.voice_chunk(Broken(), Model(), ["a", "bb"])
        self.assertEqual(waves, ["one:a", "one:bb"])
        self.assertIsNone(many)
        self.assertIn("batching failed on 2 line(s) that work one at a time (IndexError", said)

        class Fine:
            def synthesize(self, texts):
                return [ValueError("a bad line") if "bad" in t else f"many:{t}" for t in texts]

        fine = Fine()
        waves, many, said = batch.voice_chunk(fine, Model(), ["a", "bad"])
        self.assertEqual(waves[0], "many:a")
        self.assertIsInstance(waves[1], ValueError)
        self.assertIs(many, fine, "a line that fails either way doesn't turn batching off")
        self.assertIsNone(said)

        class Crashes:
            def synthesize(self, texts):
                raise RuntimeError("shapes")

        waves, many, said = batch.voice_chunk(Crashes(), Model(), ["a"])
        self.assertEqual((waves, many), (["one:a"], None))
        self.assertIn("batching failed (RuntimeError: shapes)", said)

    def test_cards_the_build_runs_by_compatibility_are_accepted(self):
        cu124 = ["sm_50", "sm_60", "sm_70", "sm_75", "sm_80", "sm_86", "sm_90"]
        cu128 = ["sm_75", "sm_80", "sm_86", "sm_90", "sm_100", "sm_120", "compute_120"]
        ok = batch.arch_supported
        self.assertTrue(ok(8, 9, cu124), "an RTX 40-series runs sm_86 kernels")
        self.assertTrue(ok(8, 9, cu128))
        self.assertTrue(ok(8, 6, cu124))
        self.assertTrue(ok(9, 0, cu124))
        self.assertFalse(ok(12, 0, cu124), "no Blackwell kernels in a CUDA 12.4 build")
        self.assertTrue(ok(12, 0, cu128))
        self.assertFalse(ok(10, 0, ["sm_80", "sm_90"]))
        self.assertTrue(ok(10, 0, ["sm_80", "compute_90"]), "PTX compiles up to newer cards")
        self.assertFalse(ok(7, 5, ["sm_80", "compute_80"]))

    def run_batch(self, n=12):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "in").mkdir()
        with open(tmp / "in" / "lines.jsonl", "w") as f:
            for i in range(n):
                f.write(json.dumps({"key": f"k{i:02d}", "text": f"Line {i}, coming up."}) + "\n")
        log = io.StringIO()
        with contextlib.redirect_stdout(log):
            code = batch.main(["--in", str(tmp / "in"), "--out", str(tmp / "out"), "--workers", "3"])
        return code, tmp / "out", log.getvalue()

    def test_every_line_is_made_on_several_copies(self):
        code, out, log = self.run_batch()
        self.assertEqual(code, 0, log)
        self.assertEqual(len(list(out.glob("*.wav"))), 12)
        self.assertEqual(json.loads((out / "voice.json").read_text())["voice"], "ip:US_James")
        self.assertEqual(json.loads((out / "stats.json").read_text())["workers"], 3)

    def test_lines_go_to_the_model_in_batches(self):

        class Many:
            def __init__(self, backend):
                self.backend = backend

            def synthesize(self, texts):
                return [self.backend.synthesize(t) for t in texts]

        server.StubBackend.model = "ming"
        old = batch.MingBatch
        batch.MingBatch = Many
        try:
            code, out, log = self.run_batch(40)
        finally:
            batch.MingBatch = old
            del server.StubBackend.model
        self.assertEqual(code, 0, log)
        self.assertEqual(len(list(out.glob("*.wav"))), 40)
        self.assertNotIn("batching", log, "the batched path ran without falling back")

    def test_a_cuda_error_stops_the_run_at_once(self):
        def broken(self, text):
            raise RuntimeError("CUDA error: no kernel image is available for execution on the device")

        old = server.StubBackend.synthesize
        server.StubBackend.synthesize = broken
        try:
            code, out, log = self.run_batch(200)
        finally:
            server.StubBackend.synthesize = old
        self.assertEqual(code, 1)
        self.assertIn("stopping: the GPU failed", log)
        self.assertEqual(list(out.glob("*.wav")), [])

    def test_lines_that_all_fail_stop_early_and_errors_are_not_repeated(self):
        def bad(self, text):
            raise ValueError("bad text\nwith a long traceback")

        old = server.StubBackend.synthesize
        server.StubBackend.synthesize = bad
        try:
            code, _, log = self.run_batch(200)
        finally:
            server.StubBackend.synthesize = old
        self.assertEqual(code, 1)
        self.assertIn("lines all failed", log)
        self.assertEqual(log.count("failed k"), 1, log)
        # The first failure's traceback once, in full; after that, only the summary.
        self.assertEqual(log.count("Traceback (most recent call last)"), 1, log)
        self.assertIn('raise ValueError("bad text', log)
        self.assertLess(log.count("ValueError"), 5, log)


if __name__ == "__main__":
    unittest.main()
