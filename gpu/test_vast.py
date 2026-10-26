"""Tests gpu/vast.py and gpu/assets.py against a fake vast.ai that runs "instances" as local processes:
    cd gpu && python3 -m unittest test_vast.py
"""

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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

os.environ["VAST_API_KEY"] = "test-key"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class FakeVast:
    """Enough of the vast.ai API for vast.py. Creating an instance runs its args here, with its env."""

    def __init__(self):
        self.created, self.destroyed, self.procs = [], [], {}
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
                self.reply({"offers": [{"id": 1, "gpu_name": "RTX 4090", "gpu_ram": 24564, "dph_total": 0.31,
                                        "geolocation": "Testland"}]})

            def do_PUT(self):
                b = self.body()
                fake.created.append(b)
                port = free_port()
                env = {k: v for k, v in b["env"].items() if not k.startswith("-p ")}
                env["TBX_PORT"] = str(port)
                args = [sys.executable] + b["args"][1:]
                fake.procs[7] = (subprocess.Popen(args, env={**os.environ, **env}, stdout=subprocess.DEVNULL), port)
                self.reply({"success": True, "new_contract": 7})

            def do_GET(self):
                if self.path.startswith("/api/v1/instances/"):
                    return self.reply({"instances": [{"id": i, "label": "triviabix-job"} for i in fake.procs
                                                     if i not in fake.destroyed]})
                proc, port = fake.procs[7]
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
        self.assertIn("-p 8000:8000", body["env"])
        self.assertLessEqual(self.fake.query["dph_total"]["lte"], 0.5)
        # Destroying twice (the instance already shut itself down) is fine.
        self.vast.Vast().destroy(7)

    def test_dry_run_rents_nothing(self):
        before = len(self.fake.created)
        self.assertEqual(self.vast.run_job("img", "true", "/nonexistent", dry_run=True), 0)
        self.assertEqual(len(self.fake.created), before)

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
        env = {**os.environ, "TBX_MODE": "serve", "TBX_CMD": "sleep 60", "TBX_TOKEN": "t",
               "TBX_DEADLINE": "0", "TBX_PORT": str(free_port())}
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
