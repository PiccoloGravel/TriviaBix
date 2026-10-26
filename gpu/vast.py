"""Rent a vast.ai GPU for TriviaBix's generation work, and always give it back. Python 3.9+, stdlib only.

    export VAST_API_KEY=...            # from https://cloud.vast.ai/manage-keys/ ; never put it in the repo
    python3 gpu/vast.py offers                                   # what you'd get, renting nothing
    python3 gpu/vast.py voice --server http://192.168.1.20:8080  # make all of Bix's missing lines in one batch
    python3 gpu/vast.py voice --server ... --hours 4             # also voice live lines for a game night
    python3 gpu/vast.py design --description "A tired game show host..."   # candidate voices for Bix
    python3 gpu/vast.py run --image IMAGE --cmd "python generate.py --out /out" --out /out --to ./art
    python3 gpu/vast.py list                                     # TriviaBix instances you're paying for
    python3 gpu/vast.py cleanup                                  # destroy all of them
    python3 gpu/vast.py destroy 1234                             # destroy one of them

Every command that rents prints the GPU, its price and the most it can cost, and asks first (pass --yes to
skip the question, e.g. from another script). Whatever happens (the job ends, fails, Ctrl-C, an error, or
the time limit) the instance is destroyed. As a backstop, the instance also shuts itself down at the time
limit, so it stops even if this computer goes to sleep. See gpu/README.md.

From Python, `run_job(...)` takes the same arguments as `run` and returns the job's exit code.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import secrets
import shlex
import shutil
import signal
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from http.cookiejar import CookieJar
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from assets import Store  # noqa: E402

API = os.environ.get("VAST_URL", "https://console.vast.ai").rstrip("/")
ROOT = Path(__file__).resolve().parent.parent
LABEL = "triviabix"
TTS_IMAGE = "ghcr.io/piccologravel/triviabix-tts:latest"
TTS_MODEL = "inclusionAI/Ming-3.0-tts-0.5b"
PORT = 8000
# How the voice image starts its service, with the rental's token so only the game server can use it.
VOICE_CMD = "cd /app && TTS_TOKEN=$TBX_TOKEN exec python server.py"
# Runs the Python source in $TBX_PY, which this script sends along, so a job doesn't depend on the image
# having the newest copy of it.
INLINE_PY = "python -c \"import base64,os,zlib;exec(zlib.decompress(base64.b64decode(os.environ['TBX_PY'])))\""
# With a designed (cloned) voice: wait for the clip (CLONE_BOOT), then start the service with it.
CLONE_CMD = f"cd /app && TTS_TOKEN=$TBX_TOKEN exec {INLINE_PY}"
# Designs candidate voices with the newest tts/make_voice.py.
DESIGN_CMD = f"cd /app && {INLINE_PY}"
DESIGN_OUT = "/out"
# Voices a whole backlog in one go with the newest tts/batch.py (sent along, like make_voice.py), reading the
# lines (and a cloned voice's clip) uploaded to JOB_IN.
BATCH_CMD = f"cd /app && {INLINE_PY}"
JOB_IN = "/in"
# Minutes from renting to the first line: pulling the image, downloading and loading the model.
VOICE_BOOT_MIN = 8
# Lines a minute the batch job makes on an RTX 4090 until a run has measured it (a few copies of the model,
# each voicing 16 lines side by side). A guess: one line at a time measured about 40. Saved runs'
# measurements go in the asset store's SPEEDS file, under VOICE_SPEED_KEY.
VOICE_RATE = 150
VOICE_SPEED_KEY = "voice-batched"
SPEEDS = "gpu-speeds.json"
# Candidate voices for Bix in the asset store (docs/assets.md). BIX_VOICE=clone:<key> picks one.
SAMPLE_KIND = "voice-sample"


class Abort(Exception):
    """Something the person should read, without a traceback."""


class Gone(Abort):
    """vast.ai has no such thing (404)."""


class Taken(Abort):
    """The offer was rented by someone else (or withdrawn) between pricing it and renting it."""


# How vast.ai says an offer can't be rented any more.
TAKEN = re.compile(r"no_such_ask|no longer available|not available|unavailable|already rented|said 410", re.I)
# How an instance says it can't pull its image, which waiting won't fix.
PULL_FAILED = re.compile(r"pull access denied|manifest unknown|unauthorized|authentication required|"
                         r"error response from daemon|denied:|no such image|repository does not exist", re.I)


def image_login(image: str, given: str | None = None) -> str | None:
    """vast.ai's login for pulling `image`: the one given (--login or VAST_IMAGE_LOGIN), else, for an image
    on ghcr.io, one made from GHCR_TOKEN (a read:packages token) and GHCR_USER. None for a public image."""
    if given:
        return given
    token = os.environ.get("GHCR_TOKEN", "").strip()
    if token and image.startswith("ghcr.io/"):
        user = os.environ.get("GHCR_USER", "").strip() or "PiccoloGravel"
        return f"-u {user} -p {token} ghcr.io"
    return None


def say(*args):
    print("[vast]", *args, file=sys.stderr, flush=True)


def event(name: str, **data):
    """A line the game server's admin panel reads (TBX_EVENTS=1), e.g. to show the price before renting."""
    if os.environ.get("TBX_EVENTS"):
        print("@tbx " + json.dumps({"event": name, **data}), file=sys.stderr, flush=True)


# ---------- vast.ai API ----------


class Vast:
    def __init__(self, key: str | None = None):
        self.key = key or os.environ.get("VAST_API_KEY", "").strip()
        if not self.key:
            raise Abort("Set VAST_API_KEY to your vast.ai API key (https://cloud.vast.ai/manage-keys/).")

    def call(self, method: str, path: str, body=None, query: dict | None = None, tries: int = 4):
        if not path.startswith("/api/"):
            path = "/api/v0" + path
        url = API + path
        if query:
            url += "?" + "&".join(f"{k}={urllib.parse.quote_plus(v if isinstance(v, str) else json.dumps(v))}"
                                  for k, v in query.items())
        data = None if body is None else json.dumps(body).encode()
        for attempt in range(tries):
            req = urllib.request.Request(url, data, method=method, headers={
                "Authorization": f"Bearer {self.key}", "Content-Type": "application/json",
                "User-Agent": "triviabix-gpu"})
            try:
                with urllib.request.urlopen(req, timeout=60) as res:
                    text = res.read().decode() or "{}"
                    return json.loads(text)
            except urllib.error.HTTPError as e:
                detail = e.read().decode(errors="replace")[:500]
                if e.code in (429, 500, 502, 503, 504) and attempt + 1 < tries:
                    time.sleep(2 ** attempt * 2)
                    continue
                if e.code == 404:
                    raise Gone(f"vast.ai has no {path}")
                if e.code in (401, 403):
                    raise Abort(f"vast.ai refused the API key ({e.code}): {detail}")
                raise Abort(f"vast.ai said {e.code} to {method} {path}: {detail}")
            except (urllib.error.URLError, TimeoutError) as e:
                if attempt + 1 < tries:
                    time.sleep(2 ** attempt * 2)
                    continue
                raise Abort(f"couldn't reach vast.ai: {e}")

    def offers(self, min_vram_gb: float, max_price: float, disk_gb: float, limit: int = 40) -> list:
        """On-demand single-GPU offers that can run the images, cheapest first. bf16 needs Ampere or
        newer (compute 8.0), and the images' PyTorch (2.6, CUDA 12.4) has kernels up to Hopper (9.0):
        on a newer card, such as an RTX 50-series (Blackwell, 12.0), every line fails with "no kernel
        image is available"."""
        q = {
            "verified": {"eq": True}, "external": {"eq": False}, "rentable": {"eq": True},
            "rented": {"eq": False}, "num_gpus": {"eq": 1},
            "gpu_ram": {"gte": min_vram_gb * 1000}, "compute_cap": {"gte": 800, "lte": max_compute_cap()},
            "cuda_max_good": {"gte": 12.4}, "reliability": {"gte": 0.98},
            "inet_down": {"gte": 200}, "direct_port_count": {"gte": 1},
            "disk_space": {"gte": disk_gb}, "dph_total": {"lte": max_price},
            "order": [["dph_total", "asc"]], "type": "on-demand", "limit": limit,
            "allocated_storage": disk_gb,
        }
        if gpus := gpu_names():
            q["gpu_name"] = {"in": gpus}
        return [o for o in self.call("POST", "/bundles/", q).get("offers") or [] if can_run(o)]

    def create(self, offer_id: int, body: dict) -> int:
        try:
            res = self.call("PUT", f"/asks/{offer_id}/", body, tries=1)
        except Gone as e:
            raise Taken(str(e))
        except Abort as e:
            if TAKEN.search(str(e)):
                raise Taken(str(e))
            raise
        if not res.get("success") or not res.get("new_contract"):
            raise Abort(f"vast.ai didn't start the instance: {res}")
        return int(res["new_contract"])

    def instance(self, iid: int) -> dict | None:
        return self.call("GET", f"/instances/{iid}/", query={"owner": "me"}).get("instances")

    def instances(self) -> list:
        rows, params = [], {"select_filters": {}, "order_by": [{"col": "id", "dir": "asc"}], "limit": 25}
        while True:
            page = self.call("GET", "/api/v1/instances/", query=params)
            rows += page.get("instances") or []
            if not page.get("next_token"):
                return rows
            params["after_token"] = page["next_token"]

    def ours(self) -> list:
        return [i for i in self.instances() if str(i.get("label") or "").startswith(LABEL)]

    def logs(self, iid: int, tail: int = 200, wait: float = 45) -> list:
        """The container's last `tail` lines of output. vast.ai uploads them and returns a link, which can
        take a few seconds to fill."""
        res = self.call("PUT", f"/instances/request_logs/{iid}/", {"tail": str(tail)}, tries=2)
        url = res.get("result_url")
        if not url:
            raise Abort(f"vast.ai didn't return a logs link: {res}")
        give_up = time.time() + wait
        while True:
            try:
                with urllib.request.urlopen(url, timeout=30) as r:
                    text = r.read().decode(errors="replace")
                if text.strip() or time.time() > give_up:
                    return text.splitlines()
            except (urllib.error.URLError, OSError) as e:
                if time.time() > give_up:
                    raise Abort(f"the logs link didn't work: {e}")
            time.sleep(3)

    def destroy(self, iid: int):
        for attempt in range(6):
            try:
                self.call("DELETE", f"/instances/{iid}/", {})
                say(f"destroyed instance {iid}")
                return
            except Gone:
                say(f"instance {iid} is gone")
                return
            except Abort as e:
                if attempt == 5:
                    raise Abort(f"COULDN'T DESTROY INSTANCE {iid}, it may still be billing: {e}. "
                                f"Run `python3 gpu/vast.py cleanup` or destroy it at https://cloud.vast.ai/instances/")
                time.sleep(5)


# ---------- What runs on the instance ----------

# Started as `python3 -c ...` in the image, so the image only needs Python 3. The job J (see job_blob) comes
# in one env var, TBX_JOB. The wrapper exports J's env plus TBX_TOKEN, TBX_PORT and TBX_OUT, then runs J's
# cmd; in job mode it also serves /status, /partial (output changed since a time) and the tarred output dir on
# the port behind the token, and with J's input set it first waits for PUT /input, a tarball it unpacks there. At the
# deadline, or once a finished job's result has waited `linger` seconds, it destroys its own instance
# (vast.ai gives every instance CONTAINER_ID and CONTAINER_API_KEY) or, failing that, exits, which stops
# the GPU billing.
WRAPPER = r'''
import collections, hmac, json, os, signal, subprocess, sys, tarfile, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
E = os.environ
E.update(J.get("env") or {})
mode, cmd, out, token = J["mode"], J["cmd"], J.get("out") or "/out", J["token"]
port = int(E.get("TBX_PORT") or J["port"])
E.update(TBX_TOKEN=token, TBX_PORT=str(port), TBX_OUT=out)
deadline, linger = float(J["deadline"]), float(J.get("linger", 900))
print("[tbx] wrapper up: %s mode, %s, python %s, port %s" % (mode, os.getcwd(), sys.version.split()[0], port), flush=True)
log = collections.deque(maxlen=400); seq = [0]
state = {"state": "running", "code": None}
done_at = [None]
RESULT = "/tmp/tbx-result.tgz"

def note(line):
    seq[0] += 1; log.append((seq[0], line)); print(line, flush=True)

def finish(why):
    note("[tbx] shutting down: " + why)
    try: proc.kill()
    except Exception: pass
    cid, key = E.get("CONTAINER_ID"), E.get("CONTAINER_API_KEY")
    if cid and key:
        try:
            req = urllib.request.Request("https://console.vast.ai/api/v0/instances/%s/" % cid, b"{}", method="DELETE",
                                         headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
            urllib.request.urlopen(req, timeout=30)
        except Exception as e:
            print("[tbx] self-destroy failed: %r" % e, flush=True)
    os._exit(0)

os.makedirs(out, exist_ok=True)
proc = None
inp = J.get("input")
got_input = threading.Event()
if not inp:
    got_input.set()

def start():
    global proc
    proc = subprocess.Popen(["sh", "-c", cmd], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    def pump():
        for line in proc.stdout:
            note(line.rstrip("\n"))
    threading.Thread(target=pump, daemon=True).start()

def tar_of(after):
    """The output files changed since `after`, so results can come home while the job still works."""
    path = "/tmp/tbx-partial-%d.tgz" % threading.get_ident()
    with tarfile.open(path, "w:gz") as t:
        for root, dirs, files in os.walk(out):
            for f in files:
                full = os.path.join(root, f)
                if f.endswith(".part"): continue
                try:
                    if os.path.getmtime(full) > after: t.add(full, arcname=os.path.relpath(full, out))
                except OSError: pass
    with open(path, "rb") as f: data = f.read()
    os.remove(path)
    return data

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def ok(self):
        if hmac.compare_digest(self.headers.get("Authorization", "").encode(), ("Bearer " + token).encode()):
            return True
        self.send_response(401); self.send_header("Content-Length", "0"); self.end_headers(); return False
    def body(self, data, ctype="application/json", headers=()):
        self.send_response(200); self.send_header("Content-Type", ctype)
        for k, v in headers: self.send_header(k, v)
        self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        if not self.ok(): return
        if self.path.startswith("/status"):
            since = int(self.path.partition("since=")[2] or 0)
            lines = [l for n, l in list(log) if n > since]
            return self.body(json.dumps(dict(state, seq=seq[0], log=lines, started=proc is not None)).encode())
        if self.path.startswith("/partial"):
            now = time.time() - 1
            data = tar_of(float(self.path.partition("after=")[2] or 0))
            return self.body(data, "application/gzip", [("X-Until", "%.3f" % now)])
        if self.path == "/result" and state["state"] != "running":
            with open(RESULT, "rb") as f: return self.body(f.read(), "application/gzip")
        self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()
    def do_PUT(self):
        if not self.ok(): return
        if self.path != "/input" or not inp or got_input.is_set():
            self.send_response(409); self.send_header("Content-Length", "0"); self.end_headers(); return
        size = int(self.headers.get("Content-Length", 0))
        path = "/tmp/tbx-input.tgz"
        with open(path, "wb") as f:
            while size > 0:
                chunk = self.rfile.read(min(size, 1 << 20))
                if not chunk: break
                f.write(chunk); size -= len(chunk)
        os.makedirs(inp, exist_ok=True)
        with tarfile.open(path) as t:
            for m in t.getmembers():
                if (m.isfile() or m.isdir()) and not m.name.startswith("/") and ".." not in m.name.split("/"):
                    t.extract(m, inp)
        note("[tbx] got the job's input; starting")
        self.body(b'{"ok": true}')
        got_input.set()

if mode == "job":
    srv = ThreadingHTTPServer(("0.0.0.0", port), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
signal.signal(signal.SIGTERM, lambda *a: finish("stopped"))
if inp:
    note("[tbx] waiting for the job's input")
while True:
    time.sleep(2)
    if time.time() > deadline:
        finish("time limit reached")
    if proc is None:
        if got_input.is_set():
            start()
        continue
    code = proc.poll()
    if code is not None and mode == "serve":
        time.sleep(1); finish("the service exited with %s" % code)
    if code is not None and done_at[0] is None:
        time.sleep(1)
        with tarfile.open(RESULT, "w:gz") as t: t.add(out, arcname=".")
        state.update(state="done" if code == 0 else "failed", code=code)
        note("[tbx] job exited with %s; result ready" % code)
        done_at[0] = time.time()
    if done_at[0] and time.time() > done_at[0] + linger:
        finish("nobody collected the result")
'''


def pack(source: str) -> str:
    return base64.b64encode(zlib.compress(source.encode(), 9)).decode()


def job_blob(**job) -> str:
    """Everything the wrapper needs, the wrapper's own source included, as one env value of only base64
    characters. vast.ai turns env into docker flags, and values with spaces, quotes, `&&` or `$` (like a
    job's command) don't reliably survive that."""
    return pack(json.dumps({"wrapper": WRAPPER, **job}))


# Decodes TBX_JOB and runs the wrapper in it.
BOOT = "import base64,json,os,zlib;J=json.loads(zlib.decompress(base64.b64decode(os.environ['TBX_JOB'])));exec(J['wrapper'])"


def boot_args() -> list:
    # The boot code goes as a list of byte values, with no spaces or quotes, so it reaches Python intact
    # whether vast.ai hands docker the args as they are or joins and splits them again.
    return ["python3", "-c", "exec(bytes([%s]))" % ",".join(map(str, BOOT.encode()))]


# Started in place of the voice service when Bix speaks in a cloned voice. It answers /healthz as "not
# ready" and takes the clip at POST /v1/voice (behind the rental's token), saves it with its transcript
# under /voices, then turns into the voice service speaking with it.
CLONE_BOOT = r'''
import base64, hmac, json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
E = os.environ
name = E["TBX_VOICE_NAME"]
folder = E.get("TBX_VOICES", "/voices")
path = os.path.join(folder, name + ".wav")
got = []

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        if self.path == "/healthz":
            return self.send(200, {"ok": True, "ready": False, "voice": None, "waiting": "the voice clip"})
        self.send(404, {"error": "not found"})
    def do_POST(self):
        if self.path != "/v1/voice":
            return self.send(404, {"error": "not found"})
        if not hmac.compare_digest(self.headers.get("Authorization", "").encode(), ("Bearer " + E["TBX_TOKEN"]).encode()):
            return self.send(401, {"error": "wrong or missing token"})
        size = int(self.headers.get("Content-Length", 0))
        if size > 30_000_000:
            return self.send(413, {"error": "too big"})
        try:
            body = json.loads(self.rfile.read(size))
            wav, text = base64.b64decode(body["wav"]), str(body["text"]).strip()
        except Exception as e:
            return self.send(400, {"error": repr(e)})
        os.makedirs(folder, exist_ok=True)
        with open(path, "wb") as f: f.write(wav)
        with open(path[:-4] + ".txt", "w", encoding="utf-8") as f: f.write(text + "\n")
        got.append(1)
        self.send(200, {"ok": True})

srv = HTTPServer(("0.0.0.0", int(E.get("PORT", "8000"))), H)
while not got:
    srv.handle_request()
srv.server_close()
print("[tbx] got the voice clip; starting the voice service", flush=True)
E["BIX_VOICE"] = "clone:" + path
os.execv(sys.executable, [sys.executable, "server.py"])
'''


# ---------- Renting ----------


class Rental:
    """A rented instance running `cmd` in `image` under the wrapper. A context manager: leaving it, however
    that happens, destroys the instance."""

    def __init__(self, vast: Vast, offers: list | dict, image: str, cmd: str, mode: str, minutes: float,
                 env: dict | None = None, port: int = PORT, disk: float = 40, out: str = "/out",
                 login: str | None = None, kind: str = "job", inputs: str | None = None):
        # The offers to try in order, cheapest first, all within the price limit already confirmed: one can be
        # taken by someone else between pricing it and renting it.
        self.offers = [offers] if isinstance(offers, dict) else list(offers)
        self.vast, self.offer, self.port, self.image = vast, self.offers[0], port, image
        self.token = secrets.token_urlsafe(24)
        self.deadline = time.time() + minutes * 60
        self.id = None
        self.rented_at = None
        self.body = {
            "client_id": "me", "image": image, "disk": disk, "label": f"{LABEL}-{kind}",
            "runtype": "args", "args": boot_args(), "image_login": login,
            "env": {f"-p {port}:{port}": "1",
                    "TBX_JOB": job_blob(mode=mode, cmd=cmd, out=out, token=self.token, port=port,
                                        deadline=int(self.deadline), env=dict(env or {}), input=inputs)},
        }

    def __enter__(self):
        for offer in self.offers:
            try:
                self.id = self.vast.create(offer["id"], self.body)
            except Taken:
                say(f"offer {offer['id']} ({offer.get('gpu_name')}) was just taken by someone else; trying the next")
                continue
            self.offer = offer
            break
        else:
            raise Abort(f"every suitable GPU under the price limit ({len(self.offers)} tried) was taken before it "
                        "could be rented; nothing was rented or charged. Try again in a minute")
        self.rented_at = time.time()
        say(f"rented instance {self.id}: {describe(self.offer)}")
        event("rented", instance=self.id, price_hr=self.offer.get("dph_total", 0))
        return self

    def __exit__(self, kind, err, tb):
        if self.id is not None:
            if kind is not None and not issubclass(kind, KeyboardInterrupt):
                self.show_logs()
            self.vast.destroy(self.id)
            seconds = time.time() - self.rented_at
            event("destroyed", instance=self.id, seconds=round(seconds),
                  cost=round(self.offer.get("dph_total", 0) * seconds / 3600, 4))

    def show_logs(self, lines: int = 80):
        """Puts the end of the instance's own output in our log, so a failed start says why."""
        try:
            tail = self.vast.logs(self.id)
        except Exception as e:
            say(f"couldn't get instance {self.id}'s logs: {e}")
            return
        if not tail:
            say(f"instance {self.id} logged nothing")
            return
        say(f"the last {min(len(tail), lines)} lines instance {self.id} logged:")
        for line in tail[-lines:]:
            say(f"[gpu] {line}")

    def address(self, boot_minutes: float = 20) -> str:
        """Waits for the instance to run and returns http://ip:port for the container's port."""
        give_up = min(time.time() + boot_minutes * 60, self.deadline)
        last = None
        while time.time() < give_up:
            info = self.vast.instance(self.id) or {}
            status = info.get("actual_status") or info.get("status_msg") or "starting"
            if status != last:
                say(f"instance {self.id}: {status}")
                last = status
            if info.get("actual_status") in ("exited", "offline"):
                raise Abort(f"the instance stopped while starting ({info.get('status_msg') or status})")
            msg = str(info.get("status_msg") or "")
            if info.get("actual_status") != "running" and PULL_FAILED.search(msg):
                raise Abort(f"the GPU couldn't download {self.image}: {msg.strip()[:300]}. If the image is private, "
                            "set the image registry login on the Settings page (or make the package public)")
            mapping = (info.get("ports") or {}).get(f"{self.port}/tcp") or []
            if info.get("actual_status") == "running" and info.get("public_ipaddr") and mapping:
                return f"http://{info['public_ipaddr'].strip()}:{mapping[0]['HostPort']}"
            time.sleep(10)
        raise Abort(f"the instance didn't start within {boot_minutes:g} minutes (pulling the image can be slow; "
                    "try again, or raise --boot-minutes)")

    def http(self, url: str, timeout: float = 30):
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {self.token}"})
        return urllib.request.urlopen(req, timeout=timeout)


def describe(offer: dict) -> str:
    return (f"{offer.get('gpu_name')} {offer.get('gpu_ram', 0) / 1024:.0f} GB, ${offer.get('dph_total', 0):.3f}/hr, "
            f"{offer.get('geolocation') or 'unknown location'} (offer {offer.get('id')})")


# How fast each GPU gets through our jobs next to an RTX 4090 (1.0): rough figures for bf16 inference, to
# pick the GPU that finishes a job for the least money rather than the one with the lowest hourly price.
# A GPU not listed counts as UNKNOWN_SPEED. The voice job measures its real speed and remembers it.
SPEED = {
    "RTX 3080": 0.45, "RTX 3080 Ti": 0.5, "RTX 3090": 0.55, "RTX 3090 Ti": 0.6, "RTX 4060 Ti": 0.35,
    "RTX 4070": 0.45, "RTX 4070 Ti": 0.55, "RTX 4070S Ti": 0.6, "RTX 4080": 0.75, "RTX 4080S": 0.78,
    "RTX 4090": 1.0, "RTX 4090D": 0.9, "RTX 5080": 0.85, "RTX 5090": 1.35,
    "RTX A4000": 0.3, "RTX A4500": 0.38, "RTX A5000": 0.45, "RTX A6000": 0.55, "RTX 6000Ada": 1.0,
    "A10": 0.35, "A40": 0.5, "L4": 0.3, "L40": 0.95, "L40S": 1.05,
    "A100 PCIE": 0.9, "A100X": 0.9, "A100 SXM4": 1.0, "H100 PCIE": 1.4, "H100 NVL": 1.6, "H100 SXM": 1.8,
    "H200": 1.9,
}
UNKNOWN_SPEED = 0.5


def speed(offer: dict, offload_below_gb: float = 0) -> float:
    """`offer`'s GPU's speed next to an RTX 4090. Below `offload_below_gb` of GPU memory a job has to move
    model parts in and out of it (the art models), which costs about 40%."""
    name = " ".join(str(offer.get("gpu_name") or "").split())
    s = SPEED.get(name, UNKNOWN_SPEED)
    if offload_below_gb and offer.get("gpu_ram", 0) / 1024 < offload_below_gb:
        s *= 0.6
    return s


def estimate(offer: dict, work_minutes: float, boot_minutes: float, offload_below_gb: float = 0) -> tuple[float, float]:
    """Minutes and dollars `offer` would take for a job of `work_minutes` on an RTX 4090, plus starting up."""
    minutes = boot_minutes + work_minutes / speed(offer, offload_below_gb)
    return minutes, offer.get("dph_total", 0) * minutes / 60


def pick_offers(vast: Vast, min_vram: float, max_price: float, disk: float, work_minutes: float | None = None,
                boot_minutes: float = 0, max_minutes: float | None = None, offload_below_gb: float = 0) -> list:
    """The suitable offers under `max_price`. The first is the one quoted; the rest are fallbacks. Given the
    job's size (`work_minutes` on an RTX 4090), they're ordered by what the whole job is expected to cost,
    so a faster card that finishes sooner wins over a cheaper one that takes longer, and cards that wouldn't
    finish within `max_minutes` go last. Without it, cheapest first."""
    offers = vast.offers(min_vram, max_price, disk)
    if not offers:
        raise Abort(f"no verified GPU with {min_vram:g} GB or more that the images can run (Ampere to Hopper; not "
                    f"RTX 50-series yet) is available under ${max_price:.2f}/hr right now; raise --max-price or try "
                    "again later")
    if work_minutes is None:
        return offers
    for o in offers:
        o["est_minutes"], o["est_cost"] = estimate(o, work_minutes, boot_minutes, offload_below_gb)
    late = lambda o: bool(max_minutes) and o["est_minutes"] > max_minutes  # noqa: E731
    return sorted(offers, key=lambda o: (late(o), round(o["est_cost"], 3), o["est_minutes"]))


def confirm(offer: dict, minutes: float, yes: bool, what: str):
    worst = offer.get("dph_total", 0) * minutes / 60
    expected = {}
    if "est_minutes" in offer:
        expected = {"est_minutes": round(min(offer["est_minutes"], minutes), 1),
                    "est_cost": round(offer.get("dph_total", 0) * min(offer["est_minutes"], minutes) / 60, 2)}
    event("offer", gpu=offer.get("gpu_name"), vram_gb=round(offer.get("gpu_ram", 0) / 1024),
          price_hr=offer.get("dph_total", 0), location=offer.get("geolocation"), minutes=minutes,
          max_cost=round(worst, 2), **expected)
    say(f"{what} on {describe(offer)}")
    if expected:
        say(f"expected to take about {expected['est_minutes']:.0f} minutes, about ${expected['est_cost']:.2f}")
    say(f"stops after {minutes:g} minutes at most, so it can cost at most ${worst:.2f} (plus pennies for disk)")
    if yes:
        return
    if not sys.stdin.isatty():
        raise Abort("pass --yes to rent without being asked")
    if input("[vast] rent it? [y/N] ").strip().lower() not in ("y", "yes"):
        raise Abort("not renting")


def refuse_doubles(vast: Vast, another: bool, kind: str = "job"):
    """Refuses a second instance of the same kind (a voice run while one is voicing, say); instances of other
    kinds, like art next to voice, are left alone so different jobs can run side by side."""
    running = [i for i in vast.ours() if str(i.get("label") or "") == f"{LABEL}-{kind}"]
    if running and not another:
        ids = ", ".join(str(i["id"]) for i in running)
        raise Abort(f"TriviaBix {kind} instances are already running ({ids}). Run `python3 gpu/vast.py cleanup` "
                    "first, or pass --another to rent one more.")


def on_signals():
    """Ctrl-C and `docker stop` both unwind through the finally blocks that destroy the instance."""
    def stop(*_):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)


# ---------- Commands ----------


def run_job(image: str, cmd: str, to: str, out: str = "/out", min_vram: float = 16, max_price: float = 0.5,
            max_minutes: float = 60, disk: float = 40, env: dict | None = None, login: str | None = None,
            boot_minutes: float = 20, yes: bool = False, dry_run: bool = False, another: bool = False,
            vast: Vast | None = None, work_minutes: float | None = None, boot_estimate: float = 0,
            offload_below_gb: float = 0, kind: str = "job") -> int:
    """Runs `cmd` in `image` on a rented GPU, copies the container's `out` folder into `to`, and destroys the
    instance. Returns the command's exit code. With `work_minutes` (the job's length on an RTX 4090), the GPU
    is picked by the expected cost of the whole job and the estimate is shown with the price."""
    vast = vast or Vast()
    offers = pick_offers(vast, min_vram, max_price, disk, work_minutes, boot_estimate, max_minutes, offload_below_gb)
    confirm(offers[0], max_minutes, yes or dry_run, f"would run {image}" if dry_run else f"running {image}")
    if dry_run:
        return 0
    return run_rented(vast, offers, image, cmd, to, out, max_minutes, disk, env, login, boot_minutes, another,
                      kind=kind)


def run_rented(vast: Vast, offers: list, image: str, cmd: str, to: str, out: str = "/out", max_minutes: float = 60,
               disk: float = 40, env: dict | None = None, login: str | None = None, boot_minutes: float = 20,
               another: bool = False, inputs: Path | None = None, partial=None, partial_every: float = 60,
               kind: str = "job") -> int:
    """`run_job` once the price is confirmed. `inputs`, a folder, is uploaded to the instance's /in before
    `cmd` starts, in one go. `partial(folder)`, if given, is called every `partial_every` seconds with what
    the job has written to `out` since the last call, so results come home while it works."""
    refuse_doubles(vast, another, kind)
    on_signals()
    job_inputs = JOB_IN if inputs else None
    with Rental(vast, offers, image, cmd, "job", max_minutes, env=env, disk=disk, out=out, login=login,
                kind=kind, inputs=job_inputs) as r:
        base = r.address(boot_minutes)
        say(f"job running at {base}")
        if inputs:
            send_inputs(r, base, Path(inputs), time.time() + boot_minutes * 60)
        since, status, after, pulled = 0, {}, 0.0, time.time()
        while True:
            if time.time() > r.deadline:
                raise Abort(f"the job ran past --max-minutes {max_minutes:g}; stopped it")
            try:
                status = json.load(r.http(f"{base}/status?since={since}"))
            except (urllib.error.URLError, OSError, ValueError):
                time.sleep(10)  # still starting, or a blip
                continue
            for line in status.get("log", []):
                print(line, flush=True)
            since = status.get("seq", since)
            if status.get("state") != "running":
                break
            if partial and time.time() - pulled > partial_every:
                pulled = time.time()
                after = pull_partial(r, base, after, partial)
            time.sleep(5)
        say(f"job {status['state']} (exit code {status.get('code')}); downloading {out}")
        with r.http(f"{base}/result", timeout=600) as res, tempfile.TemporaryFile() as tmp:
            shutil.copyfileobj(res, tmp)
            tmp.seek(0)
            n = extract(tmp, Path(to))
        say(f"saved {n} files into {to}")
        return int(status.get("code") or 0)


def send_inputs(r: "Rental", base: str, folder: Path, ready_by: float):
    """Uploads `folder` as one tarball to the instance's wrapper, which unpacks it in /in and starts the job."""
    buf = tempfile.TemporaryFile()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        t.add(folder, arcname=".")
    size = buf.tell()
    while True:
        buf.seek(0)
        req = urllib.request.Request(f"{base}/input", buf.read(), method="PUT", headers={
            "Authorization": f"Bearer {r.token}", "Content-Type": "application/gzip"})
        try:
            with urllib.request.urlopen(req, timeout=300):
                say(f"uploaded the job's input ({size // 1024} KB)")
                return
        except urllib.error.HTTPError as e:
            if e.code == 409:  # already has it (a retry after a lost answer)
                return
            raise Abort(f"the GPU refused the job's input ({e.code})")
        except (urllib.error.URLError, OSError):  # still starting
            pass
        if time.time() > ready_by:
            raise Abort("the GPU didn't take the job's input in time")
        time.sleep(5)


def pull_partial(r: "Rental", base: str, after: float, partial) -> float:
    """Fetches what the job wrote since `after` and hands it to `partial`. Returns the next `after`."""
    try:
        with r.http(f"{base}/partial?after={after}", timeout=300) as res, tempfile.TemporaryFile() as tmp:
            until = float(res.headers.get("X-Until") or after)
            shutil.copyfileobj(res, tmp)
            tmp.seek(0)
            with tempfile.TemporaryDirectory(prefix="triviabix-partial-") as d:
                if extract(tmp, Path(d)):
                    partial(Path(d))
            return until
    except (urllib.error.URLError, OSError, ValueError, tarfile.TarError) as e:
        say(f"couldn't fetch results so far ({e}); trying again shortly")
        return after


def extract(fileobj, dest: Path) -> int:
    """Unpacks a job's tarball, refusing anything that would land outside `dest`."""
    dest.mkdir(parents=True, exist_ok=True)
    root, n = dest.resolve(), 0
    with tarfile.open(fileobj=fileobj, mode="r:gz") as t:
        for m in t.getmembers():
            target = (root / m.name).resolve()
            if not (m.isfile() or m.isdir()) or (target != root and root not in target.parents):
                continue
            if m.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with t.extractfile(m) as src, open(target, "wb") as dst:
                shutil.copyfileobj(src, dst)
            n += 1
    return n


class Game:
    """The TriviaBix server's admin API, to point Bix's voice at the rented GPU and watch the backlog."""

    def __init__(self, base: str, password: str):
        self.base = base.rstrip("/")
        self.http = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(CookieJar()))
        try:
            self.call("POST", "/api/admin/login", {"password": password})
        except urllib.error.HTTPError as e:
            raise Abort(f"couldn't log in to {self.base}/admin ({e.code}); check ADMIN_PASSWORD")
        except urllib.error.URLError as e:
            raise Abort(f"couldn't reach the game server at {self.base}: {e.reason}")

    def call(self, method: str, path: str, body=None):
        req = urllib.request.Request(self.base + path, None if body is None else json.dumps(body).encode(),
                                     method=method, headers={"Content-Type": "application/json"})
        with self.http.open(req, timeout=30) as res:
            return json.load(res)

    def status(self) -> dict:
        return self.call("GET", "/api/admin/voice")

    def use(self, url: str | None, token: str | None = None) -> dict:
        return self.call("PUT", "/api/admin/voice", {"url": url, "token": token})

    def backlog(self, voice: str) -> list:
        """The lines waiting for audio in `voice`, as {"key", "text"}."""
        return self.call("GET", "/api/admin/voice/backlog?voice=" + urllib.parse.quote(voice))["lines"]

    def put_line(self, key: str, audio: bytes, content_type: str, voice: str, model: str | None):
        """Files one line's audio in the game server's asset store."""
        req = urllib.request.Request(f"{self.base}/api/admin/voice/lines/{key}", audio, method="PUT", headers={
            "Content-Type": content_type, "X-Voice": voice, "X-Model": model or ""})
        with self.http.open(req, timeout=60) as res:
            res.read()


# The newest GPUs the images' PyTorch can run, as vast.ai's compute_cap (major * 100 + minor * 10): 900 is
# Hopper. Raise VAST_MAX_COMPUTE_CAP once the images move to a PyTorch built for Blackwell (CUDA 12.8).
MAX_COMPUTE_CAP = 900
# Blackwell cards by name, in case an offer's compute_cap is missing or wrong.
BLACKWELL = re.compile(r"RTX 50\d0|RTX PRO \d+|\bB\d{3}\b|\bGB\d{3}\b|Blackwell", re.I)


def max_compute_cap() -> int:
    try:
        return int(os.environ.get("VAST_MAX_COMPUTE_CAP") or MAX_COMPUTE_CAP)
    except ValueError:
        return MAX_COMPUTE_CAP


def can_run(offer: dict) -> bool:
    """Whether the images can run on `offer`'s GPU at all."""
    cap = offer.get("compute_cap")
    if cap is not None and not 800 <= float(cap) <= max_compute_cap():
        return False
    return max_compute_cap() > 900 or not BLACKWELL.search(str(offer.get("gpu_name") or ""))


def gpu_names() -> list:
    """VAST_GPU_NAMES (set on the admin panel's Settings page): only rent these GPU models, e.g.
    "RTX 4090, RTX 3090". Empty means any GPU that's big enough."""
    return [n.strip() for n in os.environ.get("VAST_GPU_NAMES", "").split(",") if n.strip()]


def voice_env(bix_voice: str) -> dict:
    """The voice service's settings on the GPU, with the model picked on the Settings page if any."""
    env = {"BIX_VOICE": bix_voice, "PORT": str(PORT)}
    if os.environ.get("TTS_MODEL"):
        env["TTS_MODEL"] = os.environ["TTS_MODEL"]
    return env


def clone_clip(spec: str, store: Store | None = None) -> tuple[bytes, str, str]:
    """The clip, transcript and a file name for BIX_VOICE=clone:<value>: a voice designed on the Pipeline page
    (its asset store key), or a .wav on this machine with a .txt transcript next to it."""
    value = spec.partition(":")[2].strip()
    if re.fullmatch(r"[0-9a-f]{32}", value):
        store = store or Store()
        for wav in sorted(store.root.glob(f"{SAMPLE_KIND}/*/{value}.wav")):
            meta = Store._meta(wav.with_suffix(".json"))
            text = (meta.get("transcript") or meta.get("inputs", {}).get("text") or "").strip()
            if text:
                return wav.read_bytes(), text, value
        raise Abort(f"there's no designed voice {value} in the asset store ({store.root}); pick one on the "
                    "admin panel's Pipeline page")
    wav = Path(value)
    txt = wav.with_suffix(".txt")
    if not wav.is_file() or not txt.is_file():
        raise Abort(f"BIX_VOICE={spec}: a cloned voice needs {wav} and its transcript {txt} on this machine")
    name = re.sub(r"[^A-Za-z0-9_-]", "-", wav.stem) or "bix"
    return wav.read_bytes(), txt.read_text(encoding="utf-8").strip(), name


def send_clip(r: Rental, base: str, clip: tuple[bytes, str, str], ready_by: float):
    """Hands the rented voice service the clip to clone (CLONE_BOOT), retrying while it starts."""
    body = json.dumps({"wav": base64.b64encode(clip[0]).decode(), "text": clip[1]}).encode()
    while True:
        req = urllib.request.Request(f"{base}/v1/voice", body, method="POST", headers={
            "Authorization": f"Bearer {r.token}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=60):
                say(f"sent Bix's voice clip ({len(clip[0]) // 1024} KB)")
                return
        except urllib.error.HTTPError as e:
            raise Abort(f"the GPU refused the voice clip ({e.code})")
        except (urllib.error.URLError, OSError):  # still starting
            pass
        if time.time() > ready_by:
            raise Abort("the GPU didn't take the voice clip in time")
        time.sleep(5)


def design(description: str, texts: list[str] | str, count: int = 4, mode: str = "freeform", library: str | None = None,
           max_minutes: float = 40, min_vram: float = 16, max_price: float = 0.5, disk: float = 40,
           image: str = TTS_IMAGE, login: str | None = None, boot_minutes: float = 25, yes: bool = False,
           dry_run: bool = False, another: bool = False, vast: Vast | None = None) -> int:
    """Makes `count` candidate voices for Bix from a description (tts/make_voice.py) on a rented GPU and keeps
    them in the asset store. Candidate n says texts[n-1] in its clip (round the list again if it's short), so
    candidates sound different. Candidates already there are reused, so asking again costs nothing."""
    texts = [texts] if isinstance(texts, str) else texts
    description = " ".join(description.split())
    texts = [t for t in (" ".join(t.split()) for t in texts) if t]
    if not description or not texts:
        raise Abort("give a description of the voice and a sample line for it to say")
    store = Store(library)
    model = os.environ.get("TTS_MODEL") or TTS_MODEL
    style = f"ming-{mode}:{model}"
    wanted = {seed: {"description": description, "text": texts[(seed - 1) % len(texts)], "seed": seed}
              for seed in range(1, count + 1)}
    todo = []
    for seed, inputs in wanted.items():
        hit = store.find(SAMPLE_KIND, style, inputs)
        if hit:
            say(f"candidate {seed} is already in the asset store ({hit.meta.get('key')})")
        else:
            todo.append(seed)
    event("todo", count=len(todo), total=count, minutes=max_minutes)
    if not todo:
        say("every candidate is already made; renting nothing")
        return 0
    vast = vast or Vast()
    source = (ROOT / "tts" / "make_voice.py").read_text(encoding="utf-8")
    cmd = (f"{DESIGN_CMD} --out {DESIGN_OUT} --seeds {','.join(map(str, todo))} --mode {mode} "
           f"--description {shlex.quote(description)}"
           + "".join(f" --text {shlex.quote(wanted[seed]['text'])}" for seed in todo))
    env = {"TBX_PY": pack(source), "TTS_MODEL": model}
    if os.environ.get("HF_TOKEN"):
        env["HF_TOKEN"] = os.environ["HF_TOKEN"]
    with tempfile.TemporaryDirectory(prefix="triviabix-voice-") as tmp:
        code = run_job(image, cmd, tmp, DESIGN_OUT, min_vram, max_price, max_minutes, disk, env, login,
                       boot_minutes, yes, dry_run, another, vast, kind="design")
        if dry_run:
            return code
        saved = 0
        for seed in todo:
            wav = Path(tmp, f"bix-{seed}.wav")
            if not wav.is_file():
                say(f"candidate {seed} didn't come back")
                continue
            hit = store.put(SAMPLE_KIND, style, wanted[seed], wav.read_bytes(), ".wav", model=model,
                            description=description, transcript=wanted[seed]["text"], seed=seed, mode=mode,
                            tags=["bix", "voice-candidate"], generator="vast.ai")
            event("made", key=hit.meta["key"], seed=seed)
            saved += 1
        say(f"saved {saved} of {len(todo)} new candidate voices in the asset store")
        return code if saved == len(todo) else (code or 1)


def voice_key(bix_voice: str, clip: tuple[bytes, str, str] | None) -> str:
    """The name tts/server.py gives this voice, which names its folder in the asset store."""
    if clip:
        return f"clone:{clip[2]}.wav:{hashlib.sha256(clip[0] + clip[1].encode()).hexdigest()[:12]}"
    return bix_voice.strip() or "ip:US_James"


def voice_rate(store: Store) -> float:
    """Lines a minute the batch job makes on an RTX 4090: what the last run measured, else a guess."""
    try:
        return float(json.loads((store.root / SPEEDS).read_text())[VOICE_SPEED_KEY]["lines_per_min"])
    except (OSError, ValueError, KeyError, TypeError):
        return VOICE_RATE


def remember_rate(store: Store, offer: dict, stats: dict):
    """Keeps how fast this run went, scaled to an RTX 4090, for the next estimate."""
    secs = float(stats.get("seconds") or 0)
    if stats.get("made", 0) < 50 or secs < 60:
        return  # too short to tell
    rate = stats["made"] / (secs / 60) / speed(offer)
    try:
        path = store.root / SPEEDS
        speeds = json.loads(path.read_text()) if path.exists() else {}
        speeds[VOICE_SPEED_KEY] = {"lines_per_min": round(rate, 1), "gpu": offer.get("gpu_name"),
                           "workers": stats.get("workers"), "measured": int(time.time())}
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(speeds, indent=2) + "\n")
        say(f"measured {stats['made'] / (secs / 60):.0f} lines a minute on {offer.get('gpu_name')} with "
            f"{stats.get('workers')} copies of the model; the next estimate uses it")
    except OSError as e:
        say(f"couldn't save the measured speed: {e}")


class Uploader:
    """Files the audio a batch run sends home in the game server, each line once."""

    def __init__(self, game: Game, total: int):
        self.game, self.total, self.done, self.failed = game, total, set(), 0

    def __call__(self, folder: Path):
        meta = folder / "voice.json"
        if meta.exists():
            self.meta = json.loads(meta.read_text())
        meta = getattr(self, "meta", None)
        if not meta:
            return
        for f in sorted(folder.glob(f"*.{meta['format']}")):
            if f.stem in self.done:
                continue
            try:
                self.game.put_line(f.stem, f.read_bytes(), meta["content_type"], meta["voice"], meta.get("model"))
                self.done.add(f.stem)
            except (urllib.error.URLError, OSError) as e:
                self.failed += 1
                say(f"couldn't file line {f.stem} in the game server: {e}")
        event("backlog", lines=max(0, self.total - len(self.done)))


def voice(server: str, password: str, bix_voice: str = "ip:US_James", hours: float = 0, max_hours: float = 3,
          min_vram: float = 16, max_price: float = 0.5, disk: float = 40, image: str = TTS_IMAGE,
          login: str | None = None, boot_minutes: float = 25, yes: bool = False, dry_run: bool = False,
          another: bool = False, vast: Vast | None = None, library: str | None = None):
    """Makes every line waiting for Bix's voice in one batch job: the whole backlog goes up to a rented GPU,
    which runs several copies of the model at once (tts/batch.py), and the audio comes back while it works.
    With `hours`, instead keeps a voice service up that long for live lines during a game (serve_voice)."""
    if hours:
        return serve_voice(server, password, bix_voice, hours, max_hours, min_vram, max_price, disk, image, login,
                           boot_minutes, yes, dry_run, another, vast)
    clip = clone_clip(bix_voice) if bix_voice.startswith("clone:") else None
    store = Store(library)
    vast = vast or Vast()
    game = Game(server, password)
    key = voice_key(bix_voice, clip)
    lines = game.backlog(key)
    say(f"{len(lines)} lines waiting to be made in {key}")
    event("backlog", lines=len(lines))
    if not lines:
        say("nothing to make; not renting anything")
        return
    minutes = max_hours * 60
    rate = voice_rate(store)
    offers = pick_offers(vast, min_vram, max_price, disk, len(lines) / rate, VOICE_BOOT_MIN, minutes)
    confirm(offers[0], minutes, yes or dry_run, "would voice lines" if dry_run else "voicing lines")
    if dry_run:
        return
    with tempfile.TemporaryDirectory(prefix="triviabix-voice-") as tmp:
        inp, got = Path(tmp, "in"), Path(tmp, "out")
        inp.mkdir()
        with open(inp / "lines.jsonl", "w", encoding="utf-8") as f:
            for line in lines:
                f.write(json.dumps({"key": line["key"], "text": line["text"]}) + "\n")
        env = voice_env(bix_voice)
        if clip:
            (inp / "voice").mkdir()
            (inp / "voice" / f"{clip[2]}.wav").write_bytes(clip[0])
            (inp / "voice" / f"{clip[2]}.txt").write_text(clip[1] + "\n", encoding="utf-8")
            env["BIX_VOICE"] = f"clone:{JOB_IN}/voice/{clip[2]}.wav"
        if os.environ.get("HF_TOKEN"):
            env["HF_TOKEN"] = os.environ["HF_TOKEN"]
        env["TBX_PY"] = pack((ROOT / "tts" / "batch.py").read_text(encoding="utf-8"))
        # Stop making lines 5 minutes before the time limit, so what's made always comes home.
        stop_at = int(time.time() + minutes * 60 - 300)
        cmd = f"{BATCH_CMD} --in {JOB_IN} --out {DESIGN_OUT} --stop-at {stop_at}"
        upload = Uploader(game, len(lines))
        code = run_rented(vast, offers, image, cmd, str(got), DESIGN_OUT, minutes, disk, env, login, boot_minutes,
                          another, inputs=inp, partial=upload, kind="voice")
        upload(got)
        say(f"filed {len(upload.done)} of {len(lines)} lines in the game server")
        try:
            remember_rate(store, offers[0], json.loads((got / "stats.json").read_text()))
        except (OSError, ValueError):
            pass
        if len(upload.done) < len(lines):
            raise Abort(f"{len(lines) - len(upload.done)} lines weren't made; run it again to make the rest "
                        "(lines already made are kept)")
        return code


def serve_voice(server: str, password: str, bix_voice: str = "ip:US_James", hours: float = 0, max_hours: float = 3,
                min_vram: float = 16, max_price: float = 0.5, disk: float = 40, image: str = TTS_IMAGE,
                login: str | None = None, boot_minutes: float = 25, yes: bool = False, dry_run: bool = False,
                another: bool = False, vast: Vast | None = None):
    """Rents a GPU running Bix's voice service and points the game server at it for `hours`, to voice live
    lines during a game (and any backlog meanwhile), then points it back and destroys the instance."""
    clip = clone_clip(bix_voice) if bix_voice.startswith("clone:") else None
    vast = vast or Vast()
    game = Game(server, password)
    backlog = game.status()["backlog"]
    say(f"{backlog} lines waiting to be made")
    event("backlog", lines=backlog)
    if backlog == 0 and not hours:
        say("nothing to make; not renting anything")
        return
    minutes = (min(hours, max_hours) if hours else max_hours) * 60
    offers = pick_offers(vast, min_vram, max_price, disk)
    confirm(offers[0], minutes, yes or dry_run, "would voice lines" if dry_run else "voicing lines")
    if dry_run:
        return
    refuse_doubles(vast, another, "voice")
    on_signals()
    env = voice_env(bix_voice)
    cmd = VOICE_CMD
    if clip:
        env.update(BIX_VOICE="ip:US_James", TBX_PY=pack(CLONE_BOOT), TBX_VOICE_NAME=clip[2])
        cmd = CLONE_CMD
    with Rental(vast, offers, image, cmd, "serve",
                minutes, env=env, disk=disk, login=login, kind="voice") as r:
        base = r.address(boot_minutes)
        ready_by = time.time() + boot_minutes * 60
        if clip:
            send_clip(r, base, clip, ready_by)
        say(f"voice service at {base}; loading the model (the first start downloads about 3 GB)")
        while True:
            try:
                health = json.load(urllib.request.urlopen(f"{base}/healthz", timeout=10))
                if health.get("ready"):
                    if clip and not str(health.get("voice", "")).startswith("clone:"):
                        raise Abort(f"the voice service came up as {health.get('voice')}, not the cloned voice")
                    break
                if health.get("error"):
                    raise Abort(f"the voice service failed to load: {health['error']}")
            except (urllib.error.URLError, OSError, ValueError):
                pass
            if time.time() > ready_by:
                raise Abort(f"the voice service wasn't ready within {boot_minutes:g} minutes")
            time.sleep(10)
        try:
            game.use(base, r.token)
            say("the game server is using the rented GPU for Bix's voice")
            until = time.time() + hours * 3600 if hours else None
            idle = 0
            while time.time() < r.deadline - 30:
                s = game.status()
                event("backlog", lines=s["backlog"])
                say(f"{s['backlog']} lines to go" + (f", live until {time.strftime('%H:%M', time.localtime(until))}"
                                                     if until else ""))
                idle = idle + 1 if s["backlog"] == 0 and s["in_flight"] == 0 else 0
                if until is None and idle >= 2:
                    say("all lines made")
                    break
                if until is not None and time.time() >= until:
                    break
                time.sleep(15)
            else:
                say(f"reached --max-hours {max_hours:g}; stopping")
        finally:
            try:
                game.use(None)
                say("the game server is back on its own voice settings; lines already made keep playing")
            except Exception as e:
                say(f"couldn't switch the game server back ({e}); it will just use cached lines")


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="command", required=True)

    def renting(sp, vram, price):
        sp.add_argument("--min-vram", type=float, default=vram, help=f"GB of GPU memory (default {vram:g})")
        sp.add_argument("--max-price", type=float, default=price, help=f"$/hour, all in (default {price:g})")
        sp.add_argument("--disk", type=float, default=40, help="GB of disk (default 40)")
        sp.add_argument("--login", default=os.environ.get("VAST_IMAGE_LOGIN"),
                        help='for a private image: "-u USER -p TOKEN ghcr.io" (or VAST_IMAGE_LOGIN, or GHCR_TOKEN '
                             'and GHCR_USER for ghcr.io)')
        sp.add_argument("--yes", action="store_true", help="rent without asking")
        sp.add_argument("--dry-run", action="store_true", help="show the GPU and price, rent nothing")
        sp.add_argument("--another", action="store_true",
                        help="rent even if a TriviaBix instance of the same kind is running")

    o = sub.add_parser("offers", help="list the cheapest suitable GPUs")
    o.add_argument("--min-vram", type=float, default=16)
    o.add_argument("--max-price", type=float, default=0.5)
    o.add_argument("--disk", type=float, default=40)

    v = sub.add_parser("voice", help="make Bix's missing voice lines on a rented GPU")
    v.add_argument("--server", default=os.environ.get("TRIVIABIX_URL"), help="the game server, e.g. http://192.168.1.20:8080")
    v.add_argument("--voice", default=os.environ.get("BIX_VOICE", "ip:US_James"))
    v.add_argument("--hours", type=float, default=0,
                   help="keep a voice service up this long for live lines (default: voice the backlog in one batch)")
    v.add_argument("--max-hours", type=float, default=3, help="hard limit (default 3)")
    v.add_argument("--image", default=TTS_IMAGE)
    v.add_argument("--boot-minutes", type=float, default=25)
    v.add_argument("--library", help="the asset store, where measured speeds are kept (default $ASSETS_DIR)")
    renting(v, 16, 0.5)

    g = sub.add_parser("design", help="make candidate voices for Bix on a rented GPU, kept in the asset store")
    g.add_argument("--description", required=True, help="what Bix should sound like")
    g.add_argument("--text", action="append", required=True,
                   help="what he says in a candidate's clip; repeat for a different line per candidate")
    g.add_argument("--count", type=int, default=4, help="how many candidates (default 4, at most 8)")
    g.add_argument("--mode", choices=("freeform", "design"), default="freeform")
    g.add_argument("--library", help="the asset store (default $ASSETS_DIR)")
    g.add_argument("--max-minutes", type=float, default=40, help="hard limit (default 40)")
    g.add_argument("--image", default=TTS_IMAGE)
    g.add_argument("--boot-minutes", type=float, default=25)
    renting(g, 16, 0.5)

    r = sub.add_parser("run", help="run a generation job on a rented GPU and copy its output back")
    r.add_argument("--image", required=True)
    r.add_argument("--cmd", required=True, help="shell command to run in the image")
    r.add_argument("--out", default="/out", help="folder in the container to copy back (default /out)")
    r.add_argument("--to", required=True, help="local folder to copy it into")
    r.add_argument("--max-minutes", type=float, default=60, help="hard limit (default 60)")
    r.add_argument("--boot-minutes", type=float, default=20)
    r.add_argument("--env", action="append", default=[], metavar="KEY=VALUE")
    r.add_argument("--work-minutes", type=float,
                   help="how long the job takes on an RTX 4090, to pick the GPU by the whole job's cost and show it")
    r.add_argument("--boot-estimate", type=float, default=0, help="minutes to start up, for the estimate")
    r.add_argument("--offload-below", type=float, default=0,
                   help="GB of GPU memory under which the job runs slower (models moved in and out), for the estimate")
    r.add_argument("--kind", default="job",
                   help="what the job makes, e.g. art; one job of each kind can run at a time (default job)")
    renting(r, 16, 0.5)

    sub.add_parser("list", help="TriviaBix instances on your account")
    sub.add_parser("cleanup", help="destroy every TriviaBix instance on your account")
    d = sub.add_parser("destroy", help="destroy one TriviaBix instance by id")
    d.add_argument("id", type=int)
    args = p.parse_args(argv)
    if hasattr(args, "login"):
        args.login = image_login(args.image, args.login)

    try:
        if args.command == "offers":
            for offer in Vast().offers(args.min_vram, args.max_price, args.disk):
                print(describe(offer))
        elif args.command == "voice":
            if not args.server:
                raise Abort("pass --server with the game server's address (or set TRIVIABIX_URL)")
            password = os.environ.get("ADMIN_PASSWORD")
            if not password:
                raise Abort("set ADMIN_PASSWORD to the game server's admin password")
            voice(args.server, password, args.voice, args.hours, args.max_hours, args.min_vram, args.max_price,
                  args.disk, args.image, args.login, args.boot_minutes, args.yes, args.dry_run, args.another,
                  library=args.library)
        elif args.command == "design":
            if not 1 <= args.count <= 8:
                raise Abort("make 1 to 8 candidates at a time")
            return design(args.description, args.text, args.count, args.mode, args.library, args.max_minutes,
                          args.min_vram, args.max_price, args.disk, args.image, args.login, args.boot_minutes,
                          args.yes, args.dry_run, args.another)
        elif args.command == "run":
            env = dict(e.split("=", 1) for e in args.env)
            return run_job(args.image, args.cmd, args.to, args.out, args.min_vram, args.max_price,
                           args.max_minutes, args.disk, env, args.login, args.boot_minutes, args.yes,
                           args.dry_run, args.another, work_minutes=args.work_minutes,
                           boot_estimate=args.boot_estimate, offload_below_gb=args.offload_below, kind=args.kind)
        elif args.command == "list":
            for i in Vast().ours():
                up = (time.time() - (i.get("start_date") or time.time())) / 3600
                print(f"{i['id']}  {i.get('label')}  {i.get('actual_status')}  {i.get('gpu_name')}  "
                      f"${i.get('dph_total', 0):.3f}/hr  up {up:.1f}h")
        elif args.command == "cleanup":
            vast = Vast()
            ours = vast.ours()
            for i in ours:
                vast.destroy(i["id"])
            say(f"destroyed {len(ours)} instance(s)")
        elif args.command == "destroy":
            vast = Vast()
            if not any(i["id"] == args.id for i in vast.ours()):
                say(f"instance {args.id} isn't a running TriviaBix instance; nothing to do")
            else:
                vast.destroy(args.id)
        return 0
    except Abort as e:
        say(str(e))
        return 1
    except KeyboardInterrupt:
        say("stopped")
        return 130


if __name__ == "__main__":
    sys.exit(main())
