"""Tests gpu/vast.py and gpu/assets.py against a fake vast.ai that runs "instances" as local processes:
    cd gpu && python3 -m unittest test_vast.py
"""

import base64
import io
import json
import os
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
import unittest.mock
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

os.environ["VAST_API_KEY"] = "test-key"


def job(body: dict) -> dict:
    """The job a rental hands its instance in TBX_JOB."""
    return json.loads(zlib.decompress(base64.b64decode(body["env"]["TBX_JOB"])))


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class FakeVast:
    """Enough of the vast.ai API for vast.py. Creating an instance runs its args here, with its env."""

    def __init__(self):
        self.created, self.destroyed, self.procs = [], [], {}
        # Offer ids someone else rents first, what instances say while starting, and a status that makes
        # them stop while starting.
        self.taken, self.status_msg, self.stopped = set(), None, None
        self.logs = tempfile.mkdtemp(prefix="fake-vast-logs-")
        self.offers = [{"id": 1, "gpu_name": "RTX 4090", "gpu_ram": 24564, "dph_total": 0.31,
                        "geolocation": "Testland"}]
        fake = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def reply(self, obj, code=200):
                data = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def body(self):
                return json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")

            def do_POST(self):
                assert self.headers["Authorization"] == "Bearer test-key"
                q = self.body()
                fake.query = q
                self.reply({"offers": fake.offers})

            def do_PUT(self):
                b = self.body()
                if self.path.startswith("/api/v0/instances/request_logs/"):
                    fake.log_tail = int(b["tail"])
                    return self.reply({"success": True, "result_url": f"{fake.url}/logs/7.txt"})
                ask = int(self.path.rstrip("/").rsplit("/", 1)[1])
                if ask in fake.taken:
                    return self.reply({"success": False, "error": "no_such_ask",
                                       "msg": f"Instance type by id {ask} is not available."}, 400)
                fake.created.append(b)
                port = free_port()
                env = {k: v for k, v in b["env"].items() if not k.startswith("-p ")}
                env["TBX_PORT"] = str(port)
                args = [sys.executable] + b["args"][1:]
                with open(os.path.join(fake.logs, "7.txt"), "w") as log:
                    fake.procs[7] = (subprocess.Popen(args, env={**os.environ, **env}, stdout=log,
                                                      stderr=subprocess.STDOUT), port)
                self.reply({"success": True, "new_contract": 7})

            def do_GET(self):
                if self.path.startswith("/api/v1/instances/"):
                    return self.reply({"instances": [{"id": i, "label": "triviabix-job"} for i in fake.procs
                                                     if i not in fake.destroyed]})
                if self.path.startswith("/logs/"):
                    data = Path(fake.logs, "7.txt").read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    return self.wfile.write(data)
                proc, port = fake.procs[7]
                if fake.stopped:
                    return self.reply({"instances": {"actual_status": "exited", "status_msg": fake.stopped}})
                if fake.status_msg:
                    return self.reply({"instances": {"actual_status": "loading", "status_msg": fake.status_msg}})
                self.reply({"instances": {"actual_status": "running", "public_ipaddr": "127.0.0.1",
                                          "ports": {"8000/tcp": [{"HostIp": "0.0.0.0", "HostPort": str(port)}]}}})

            def do_DELETE(self):
                iid = int(self.path.rstrip("/").rsplit("/", 1)[1])
                if iid not in fake.procs or iid in fake.destroyed:
                    return self.reply({"error": "no such instance"}, 404)
                fake.destroyed.append(iid)
                fake.procs[iid][0].kill()
                self.reply({"success": True})

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fake = FakeVast()
        os.environ["VAST_URL"] = cls.fake.url
        sys.path.insert(0, str(Path(__file__).parent))
        import vast
        vast.API = cls.fake.url
        vast.time.sleep = lambda s, _sleep=vast.time.sleep: _sleep(min(s, 0.2))
        cls.vast = vast


class VastTest(Base):
    def test_a_job_runs_its_output_comes_back_and_the_instance_is_destroyed(self):
        with tempfile.TemporaryDirectory() as box, tempfile.TemporaryDirectory() as to:
            out = os.path.join(box, "out")
            cmd = f"echo rendering; mkdir -p {out}/img && printf png > {out}/img/bix.png; exit 3"
            code = self.vast.run_job("ghcr.io/x/art", cmd, to, out=out, yes=True, max_minutes=2)
            self.assertEqual(code, 3)
            self.assertEqual(Path(to, "img", "bix.png").read_text(), "png")
        self.assertIn(7, self.fake.destroyed)
        body = self.fake.created[-1]
        self.assertEqual((body["runtype"], body["image"]), ("args", "ghcr.io/x/art"))
        self.assertEqual(sorted(body["env"]), ["-p 8000:8000", "TBX_JOB"])
        self.assertRegex(body["env"]["TBX_JOB"], r"^[A-Za-z0-9+/=]+$")
        self.assertEqual(job(body)["cmd"], cmd)
        self.assertRegex(" ".join(body["args"]), r"^python3 -c exec\(bytes\(\[[0-9,]+\]\)\)$")
        self.assertLessEqual(self.fake.query["dph_total"]["lte"], 0.5)
        # Destroying twice (the instance already shut itself down) is fine.
        self.vast.Vast().destroy(7)

    def test_a_taken_offer_falls_back_to_the_next_cheapest(self):
        cheap = dict(self.fake.offers[0])
        self.fake.offers = [{**cheap, "id": 5, "dph_total": 0.20}, {**cheap, "id": 6, "dph_total": 0.25}, cheap]
        self.fake.taken = {5, 6}
        err = io.StringIO()
        os.environ["TBX_EVENTS"] = "1"
        try:
            with tempfile.TemporaryDirectory() as box, tempfile.TemporaryDirectory() as to, \
                    unittest.mock.patch("sys.stderr", err):
                code = self.vast.run_job("img", "true", to, out=box, yes=True, max_minutes=2)
        finally:
            del os.environ["TBX_EVENTS"]
            self.fake.offers, self.fake.taken = [cheap], set()
        self.assertEqual(code, 0)
        log = err.getvalue()
        self.assertIn("offer 5 (RTX 4090) was just taken", log)
        rented = next(json.loads(l[5:]) for l in log.splitlines() if l.startswith('@tbx {"event": "rented"'))
        self.assertEqual(rented["price_hr"], 0.31)

    def test_nothing_is_rented_when_every_offer_is_taken(self):
        self.fake.taken = {1}
        try:
            with self.assertRaisesRegex(self.vast.Abort, "nothing was rented"):
                self.vast.run_job("img", "true", "/nonexistent", yes=True, max_minutes=2)
        finally:
            self.fake.taken = set()

    def test_an_image_that_cannot_be_pulled_stops_at_once(self):
        self.fake.status_msg = "Error response from daemon: Head https://ghcr.io/v2/x/tts: denied: denied"
        try:
            with self.assertRaisesRegex(self.vast.Abort, "couldn't download ghcr.io/x/tts.*Settings"):
                self.vast.run_job("ghcr.io/x/tts", "sleep 30", "/nonexistent", yes=True, max_minutes=2,
                                  another=True)
        finally:
            self.fake.status_msg = None
        self.assertIn(7, self.fake.destroyed)

    def test_an_instance_that_stops_while_starting_shows_its_logs(self):
        self.fake.stopped = "success, running ghcr.io/x/tts"
        err = io.StringIO()
        try:
            with unittest.mock.patch("sys.stderr", err), \
                    self.assertRaisesRegex(self.vast.Abort, "stopped while starting"):
                # The wrapper starts (and logs) before vast.py sees the instance stopped.
                self.vast.run_job("ghcr.io/x/tts", "echo the job ran; sleep 30", "/nonexistent", yes=True,
                                  max_minutes=2, another=True)
        finally:
            self.fake.stopped = None
        log = err.getvalue()
        self.assertEqual(self.fake.log_tail, 200)
        self.assertRegex(log, r"\[gpu\] \[tbx\] wrapper up: job mode")
        self.assertIn(7, self.fake.destroyed)
        # Fetched before the instance is destroyed (instance 7 can already be gone from an earlier test).
        self.assertRegex(log, r"(?s)\[gpu\] \[tbx\] wrapper up.*(destroyed instance 7|instance 7 is gone)")

    def test_only_the_chosen_gpus_are_rented(self):
        with unittest.mock.patch.dict(os.environ, {"VAST_GPU_NAMES": "RTX 4090, RTX 3090 ,"}):
            self.vast.Vast().offers(16, 0.5, 40)
        self.assertEqual(self.fake.query["gpu_name"], {"in": ["RTX 4090", "RTX 3090"]})
        self.vast.Vast().offers(16, 0.5, 40)
        self.assertNotIn("gpu_name", self.fake.query)

    def test_dry_run_rents_nothing(self):
        before = len(self.fake.created)
        self.assertEqual(self.vast.run_job("img", "true", "/nonexistent", dry_run=True), 0)
        self.assertEqual(len(self.fake.created), before)

    def test_the_command_line_runs_jobs_and_reports_the_price(self):
        # `run --cmd` once shadowed the subcommand's name, so `run` silently did nothing.
        before = len(self.fake.created)
        err = io.StringIO()
        os.environ["TBX_EVENTS"] = "1"
        try:
            with unittest.mock.patch("sys.stderr", err):
                code = self.vast.main(["run", "--image", "img", "--cmd", "true", "--to", "/nonexistent",
                                       "--dry-run", "--max-minutes", "30"])
        finally:
            del os.environ["TBX_EVENTS"]
        self.assertEqual(code, 0)
        self.assertEqual(len(self.fake.created), before)
        events = [json.loads(l[5:]) for l in err.getvalue().splitlines() if l.startswith("@tbx ")]
        offer = next(e for e in events if e["event"] == "offer")
        self.assertEqual(offer["minutes"], 30)
        self.assertAlmostEqual(offer["max_cost"], round(offer["price_hr"] / 2, 2))

    def test_a_ghcr_token_logs_in_to_private_images(self):
        login = self.vast.image_login
        with unittest.mock.patch.dict(os.environ, {"GHCR_TOKEN": "ghp_x", "GHCR_USER": ""}):
            self.assertEqual(login("ghcr.io/piccologravel/triviabix-tts:latest"),
                             "-u PiccoloGravel -p ghp_x ghcr.io")
            self.assertIsNone(login("docker.io/library/python:3.12"))
            self.assertEqual(login("ghcr.io/a/b", "-u me -p t ghcr.io"), "-u me -p t ghcr.io")
        with unittest.mock.patch.dict(os.environ, {"GHCR_TOKEN": ""}):
            self.assertIsNone(login("ghcr.io/piccologravel/triviabix-tts:latest"))

    def test_extract_stays_inside(self):
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as t:
            for name in ("ok.txt", "../evil.txt", "/abs.txt"):
                info = tarfile.TarInfo(name)
                info.size = 2
                t.addfile(info, io.BytesIO(b"hi"))
        buf.seek(0)
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(self.vast.extract(buf, Path(d, "x")), 1)
            self.assertFalse(Path(d, "evil.txt").exists())

    def test_wrapper_stops_at_its_deadline(self):
        blob = self.vast.job_blob(mode="serve", cmd="sleep 60", token="t", deadline=0, port=free_port())
        env = {**os.environ, "TBX_JOB": blob}
        env.pop("CONTAINER_API_KEY", None)
        proc = subprocess.run([sys.executable] + self.vast.boot_args()[1:], env=env, timeout=20,
                              capture_output=True, text=True)
        self.assertIn("time limit reached", proc.stdout)


class FakeGame:
    """The game server's admin voice API: a backlog that drains while a voice server is set."""

    def __init__(self):
        self.backlog, self.servers = 3, []
        fake = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def reply(self, obj, cookie=False):
                data = json.dumps(obj).encode()
                self.send_response(200)
                if cookie:
                    self.send_header("Set-Cookie", "tbx_admin=ok; Path=/; HttpOnly")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def status(self):
                if fake.servers and fake.servers[-1]["url"]:
                    fake.backlog = max(0, fake.backlog - 1)
                return {"backlog": fake.backlog, "in_flight": 0, "server": None}

            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.reply({"ok": True}, cookie=True)

            def do_GET(self):
                assert "tbx_admin=ok" in self.headers.get("Cookie", "")
                self.reply(self.status())

            def do_PUT(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if body["url"]:
                    # The rented voice service wants the token it was started with.
                    import urllib.request
                    req = urllib.request.Request(body["url"] + "/v1/speech", json.dumps({"text": "Hi"}).encode(),
                                                 {"Authorization": "Bearer " + body["token"]})
                    body["spoke"] = urllib.request.urlopen(req).status
                fake.servers.append(body)
                self.reply(self.status())

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()


class VoiceTest(Base):
    def test_voice_makes_the_backlog_then_hands_back(self):
        tts = Path(__file__).resolve().parent.parent / "tts"
        self.vast.VOICE_CMD = (f"cd {tts} && TTS_BACKEND=stub TTS_TOKEN=$TBX_TOKEN PORT=$TBX_PORT "
                               f"exec {sys.executable} server.py")
        self.fake.destroyed.clear()
        game = FakeGame()
        self.vast.voice(game.url, "pw", yes=True, max_hours=0.1)
        self.assertEqual(game.backlog, 0)
        self.assertEqual(game.servers[0]["spoke"], 200)
        self.assertIsNone(game.servers[-1]["url"], "the game goes back to its own voice")
        self.assertEqual(self.fake.destroyed, [7])
        self.assertNotIn("TTS_MODEL", job(self.fake.created[-1])["env"])
        with unittest.mock.patch.dict(os.environ, {"TTS_MODEL": "someone/other-tts"}):
            self.assertEqual(self.vast.voice_env("ip:US_James")["TTS_MODEL"], "someone/other-tts")


class CloneTest(Base):
    def test_a_designed_voice_is_cloned_on_the_gpu(self):
        import assets
        tts = Path(__file__).resolve().parent.parent / "tts"
        with tempfile.TemporaryDirectory() as lib, tempfile.TemporaryDirectory() as voices:
            store = assets.Store(lib)
            inputs = {"description": "tired host", "text": "Hello there.", "seed": 1}
            hit = store.put(self.vast.SAMPLE_KIND, "ming-freeform:x", inputs, b"RIFFfake", ".wav",
                            transcript="Hello there.")
            key = hit.meta["key"]
            self.assertEqual(self.vast.clone_clip(f"clone:{key}", store), (b"RIFFfake", "Hello there.", key))
            with self.assertRaises(self.vast.Abort):
                self.vast.clone_clip("clone:" + "0" * 32, store)
            self.vast.CLONE_CMD = (f"cd {tts} && TTS_BACKEND=stub TTS_TOKEN=$TBX_TOKEN PORT=$TBX_PORT "
                                   f"TBX_VOICES={voices} exec {sys.executable} -c "
                                   "\"import base64,os,zlib;exec(zlib.decompress(base64.b64decode(os.environ['TBX_PY'])))\"")
            self.fake.destroyed.clear()
            game = FakeGame()
            with unittest.mock.patch.dict(os.environ, {"ASSETS_DIR": lib}):
                self.vast.voice(game.url, "pw", f"clone:{key}", yes=True, max_hours=0.1)
            self.assertEqual(Path(voices, f"{key}.wav").read_bytes(), b"RIFFfake")
            self.assertEqual(Path(voices, f"{key}.txt").read_text().strip(), "Hello there.")
            self.assertEqual(game.servers[0]["spoke"], 200)
            self.assertEqual(game.backlog, 0)
            self.assertEqual(self.fake.destroyed, [7])


class DesignTest(Base):
    def test_candidates_are_made_once_and_kept(self):
        import assets
        with tempfile.TemporaryDirectory() as lib, tempfile.TemporaryDirectory() as root, \
                tempfile.TemporaryDirectory() as box:
            # Stands in for tts/make_voice.py: writes bix-<seed>.wav for each --seeds into --out.
            Path(root, "tts").mkdir()
            Path(root, "tts", "make_voice.py").write_text(
                "import argparse, os\n"
                "p = argparse.ArgumentParser(); [p.add_argument(a) for a in ('--out', '--seeds', '--mode', "
                "'--description')]; p.add_argument('--text', action='append')\n"
                "a = p.parse_args(); os.makedirs(a.out, exist_ok=True)\n"
                "for n, t in zip(a.seeds.split(','), a.text):\n"
                "    open(os.path.join(a.out, f'bix-{n}.wav'), 'w').write(a.description + '|' + n + '|' + t)\n")
            out = os.path.join(box, "out")
            with unittest.mock.patch.multiple(self.vast, ROOT=Path(root), DESIGN_OUT=out, DESIGN_CMD=(
                    f"{sys.executable} -c "
                    "\"import base64,os,zlib;exec(zlib.decompress(base64.b64decode(os.environ['TBX_PY'])))\"")):
                before = len(self.fake.created)
                lines = ["Hi, it's me. Again.", "Still here, folks."]
                code = self.vast.design("A tired host's voice", lines, count=2, library=lib,
                                        yes=True, max_minutes=2)
                self.assertEqual(code, 0)
                self.assertEqual(len(self.fake.created), before + 1)
                store = assets.Store(lib)
                made = [a for a in store.all() if a.meta["kind"] == self.vast.SAMPLE_KIND]
                self.assertEqual(sorted(a.path.read_text() for a in made),
                                 ["A tired host's voice|1|Hi, it's me. Again.", "A tired host's voice|2|Still here, folks."])
                self.assertEqual({a.meta["transcript"] for a in made}, set(lines))
                # Asking again rents nothing; one more candidate rents for that one only.
                self.assertEqual(self.vast.design("A tired host's voice", lines, count=2,
                                                  library=lib, yes=True), 0)
                self.assertEqual(len(self.fake.created), before + 1)
                self.vast.design("A tired host's voice", lines, count=3, library=lib, yes=True,
                                 max_minutes=2)
                cmd = job(self.fake.created[-1])["cmd"]
                self.assertIn("--seeds 3 ", cmd)
                self.assertTrue(cmd.endswith("--text 'Hi, it'\"'\"'s me. Again.'"), cmd)


class AssetsTest(unittest.TestCase):
    def test_put_find_index_import(self):
        import assets
        with tempfile.TemporaryDirectory() as d:
            store = assets.Store(d)
            inputs = {"prompt": "Bix bowing", "seed": 7}
            self.assertIsNone(store.find("image", "flux:v1", inputs))
            a = store.put("image", "flux:v1", inputs, b"png", ".png", tags=["cutscene"], rating="PG")
            self.assertEqual(store.find("image", "flux:v1", dict(reversed(inputs.items()))).path, a.path)
            self.assertIsNone(store.find("image", "flux:v2", inputs))
            # Matches the game server's voice keys (server/src/voice.rs).
            self.assertEqual(assets.key_of({"text": "Hello"}), "ef73ae6a8b47cd3601ab3fad1fb8099e")
            job = Path(d, "job")
            job.mkdir()
            (job / "a.png").write_bytes(b"A")
            (job / "a.png.json").write_text(json.dumps({"inputs": {"prompt": "a"}, "tags": ["x"]}))
            assets.main(["--dir", d, "import", str(job), "--kind", "image", "--style", "flux:v1"])
            assets.main(["--dir", d, "import", str(job), "--kind", "image", "--style", "flux:v1"])
            index = json.loads(Path(d, "index.json").read_text())
            self.assertEqual(sorted(e["inputs"]["prompt"] for e in index), ["Bix bowing", "a"])


if __name__ == "__main__":
    unittest.main()
