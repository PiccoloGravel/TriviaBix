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
        self.assertLess(log.count("ValueError"), 4, log)


if __name__ == "__main__":
    unittest.main()
