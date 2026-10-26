"""Rent a vast.ai GPU for TriviaBix's generation work, and always give it back. Python 3.9+, stdlib only.

    export VAST_API_KEY=...            # from https://cloud.vast.ai/manage-keys/ ; never put it in the repo
    python3 gpu/vast.py offers                                   # what you'd get, renting nothing
    python3 gpu/vast.py voice --server http://192.168.1.20:8080  # make Bix's missing lines, then stop
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
# Candidate voices for Bix in the asset store (docs/assets.md). BIX_VOICE=clone:<key> picks one.
SAMPLE_KIND = "voice-sample"


class Abort(Exception):
    """Something the person should read, without a traceback."""


class Gone(Abort):
    """vast.ai has no such thing (404)."""


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

    def offers(self, min_vram_gb: float, max_price: float, disk_gb: float, limit: int = 10) -> list:
        """On-demand single-GPU offers that can run the images, cheapest first. bf16 needs Ampere or
        newer (compute 8.0), and the images are built for CUDA 12.4."""
        q = {
            "verified": {"eq": True}, "external": {"eq": False}, "rentable": {"eq": True},
            "rented": {"eq": False}, "num_gpus": {"eq": 1},
            "gpu_ram": {"gte": min_vram_gb * 1000}, "compute_cap": {"gte": 800},
            "cuda_max_good": {"gte": 12.4}, "reliability": {"gte": 0.98},
            "inet_down": {"gte": 200}, "direct_port_count": {"gte": 1},
            "disk_space": {"gte": disk_gb}, "dph_total": {"lte": max_price},
            "order": [["dph_total", "asc"]], "type": "on-demand", "limit": limit,
            "allocated_storage": disk_gb,
        }
        if gpus := gpu_names():
            q["gpu_name"] = {"in": gpus}
        return self.call("POST", "/bundles/", q).get("offers") or []

    def create(self, offer_id: int, body: dict) -> int:
        res = self.call("PUT", f"/asks/{offer_id}/", body, tries=1)
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

# Started as `python3 -c ...` in the image, so the image only needs Python 3. It runs TBX_CMD; in job mode
# it also serves /status and the tarred TBX_OUT on TBX_PORT behind TBX_TOKEN. At TBX_DEADLINE, or once a
# finished job's result has waited TBX_LINGER seconds, it destroys its own instance (vast.ai gives every
# instance CONTAINER_ID and CONTAINER_API_KEY) or, failing that, exits, which stops the GPU billing.
WRAPPER = r'''
import collections, hmac, json, os, signal, subprocess, sys, tarfile, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
E = os.environ
mode, cmd, out, token = E["TBX_MODE"], E["TBX_CMD"], E.get("TBX_OUT", "/out"), E["TBX_TOKEN"]
deadline, linger = float(E["TBX_DEADLINE"]), float(E.get("TBX_LINGER", "900"))
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
proc = subprocess.Popen(["sh", "-c", cmd], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)

def pump():
    for line in proc.stdout:
        note(line.rstrip("\n"))
threading.Thread(target=pump, daemon=True).start()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def ok(self):
        if hmac.compare_digest(self.headers.get("Authorization", "").encode(), ("Bearer " + token).encode()):
            return True
        self.send_response(401); self.send_header("Content-Length", "0"); self.end_headers(); return False
    def body(self, data, ctype="application/json"):
        self.send_response(200); self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        if not self.ok(): return
        if self.path.startswith("/status"):
            since = int(self.path.partition("since=")[2] or 0)
            lines = [l for n, l in list(log) if n > since]
            return self.body(json.dumps(dict(state, seq=seq[0], log=lines)).encode())
        if self.path == "/result" and state["state"] != "running":
            with open(RESULT, "rb") as f: return self.body(f.read(), "application/gzip")
        self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()
if mode == "job":
    srv = ThreadingHTTPServer(("0.0.0.0", int(E["TBX_PORT"])), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
signal.signal(signal.SIGTERM, lambda *a: finish("stopped"))
while True:
    time.sleep(2)
    if time.time() > deadline:
        finish("time limit reached")
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


def boot_args() -> list:
    return ["python3", "-c", f"import base64,zlib;exec(zlib.decompress(base64.b64decode('{pack(WRAPPER)}')))"]


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

    def __init__(self, vast: Vast, offer: dict, image: str, cmd: str, mode: str, minutes: float,
                 env: dict | None = None, port: int = PORT, disk: float = 40, out: str = "/out",
                 login: str | None = None, kind: str = "job"):
        self.vast, self.offer, self.port = vast, offer, port
        self.token = secrets.token_urlsafe(24)
        self.deadline = time.time() + minutes * 60
        self.id = None
        self.rented_at = None
        self.body = {
            "client_id": "me", "image": image, "disk": disk, "label": f"{LABEL}-{kind}",
            "runtype": "args", "args": boot_args(), "image_login": login,
            "env": {f"-p {port}:{port}": "1", **(env or {}), "TBX_MODE": mode, "TBX_CMD": cmd,
                    "TBX_OUT": out, "TBX_TOKEN": self.token, "TBX_PORT": str(port),
                    "TBX_DEADLINE": str(int(self.deadline))},
        }

    def __enter__(self):
        self.id = self.vast.create(self.offer["id"], self.body)
        self.rented_at = time.time()
        say(f"rented instance {self.id}: {describe(self.offer)}")
        event("rented", instance=self.id, price_hr=self.offer.get("dph_total", 0))
        return self

    def __exit__(self, *exc):
        if self.id is not None:
            self.vast.destroy(self.id)
            seconds = time.time() - self.rented_at
            event("destroyed", instance=self.id, seconds=round(seconds),
                  cost=round(self.offer.get("dph_total", 0) * seconds / 3600, 4))

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


def pick_offer(vast: Vast, min_vram: float, max_price: float, disk: float) -> dict:
    offers = vast.offers(min_vram, max_price, disk)
    if not offers:
        raise Abort(f"no verified GPU with {min_vram:g} GB or more is available under ${max_price:.2f}/hr right "
                    "now; raise --max-price or try again later")
    return offers[0]


def confirm(offer: dict, minutes: float, yes: bool, what: str):
    worst = offer.get("dph_total", 0) * minutes / 60
    event("offer", gpu=offer.get("gpu_name"), vram_gb=round(offer.get("gpu_ram", 0) / 1024),
          price_hr=offer.get("dph_total", 0), location=offer.get("geolocation"), minutes=minutes,
          max_cost=round(worst, 2))
    say(f"{what} on {describe(offer)}")
    say(f"stops after {minutes:g} minutes at most, so it can cost at most ${worst:.2f} (plus pennies for disk)")
    if yes:
        return
    if not sys.stdin.isatty():
        raise Abort("pass --yes to rent without being asked")
    if input("[vast] rent it? [y/N] ").strip().lower() not in ("y", "yes"):
        raise Abort("not renting")


def refuse_doubles(vast: Vast, another: bool):
    running = vast.ours()
    if running and not another:
        ids = ", ".join(str(i["id"]) for i in running)
        raise Abort(f"TriviaBix instances are already running ({ids}). Run `python3 gpu/vast.py cleanup` first, "
                    "or pass --another to rent one more.")


def on_signals():
    """Ctrl-C and `docker stop` both unwind through the finally blocks that destroy the instance."""
    def stop(*_):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)


# ---------- Commands ----------


def run_job(image: str, cmd: str, to: str, out: str = "/out", min_vram: float = 16, max_price: float = 0.5,
            max_minutes: float = 60, disk: float = 40, env: dict | None = None, login: str | None = None,
            boot_minutes: float = 20, yes: bool = False, dry_run: bool = False, another: bool = False,
            vast: Vast | None = None) -> int:
    """Runs `cmd` in `image` on a rented GPU, copies the container's `out` folder into `to`, and destroys the
    instance. Returns the command's exit code."""
    vast = vast or Vast()
    offer = pick_offer(vast, min_vram, max_price, disk)
    confirm(offer, max_minutes, yes or dry_run, f"would run {image}" if dry_run else f"running {image}")
    if dry_run:
        return 0
    refuse_doubles(vast, another)
    on_signals()
    with Rental(vast, offer, image, cmd, "job", max_minutes, env=env, disk=disk, out=out, login=login) as r:
        base = r.address(boot_minutes)
        say(f"job running at {base}")
        since, status = 0, {}
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
            time.sleep(5)
        say(f"job {status['state']} (exit code {status.get('code')}); downloading {out}")
        with r.http(f"{base}/result", timeout=600) as res, tempfile.TemporaryFile() as tmp:
            shutil.copyfileobj(res, tmp)
            tmp.seek(0)
            n = extract(tmp, Path(to))
        say(f"saved {n} files into {to}")
        return int(status.get("code") or 0)


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


def design(description: str, text: str, count: int = 4, mode: str = "freeform", library: str | None = None,
           max_minutes: float = 40, min_vram: float = 16, max_price: float = 0.5, disk: float = 40,
           image: str = TTS_IMAGE, login: str | None = None, boot_minutes: float = 25, yes: bool = False,
           dry_run: bool = False, another: bool = False, vast: Vast | None = None) -> int:
    """Makes `count` candidate voices for Bix from a description (tts/make_voice.py) on a rented GPU and keeps
    them in the asset store. Candidates already there are reused, so asking again costs nothing."""
    description, text = " ".join(description.split()), " ".join(text.split())
    if not description or not text:
        raise Abort("give a description of the voice and a sample line for it to say")
    store = Store(library)
    model = os.environ.get("TTS_MODEL") or TTS_MODEL
    style = f"ming-{mode}:{model}"
    wanted = {seed: {"description": description, "text": text, "seed": seed} for seed in range(1, count + 1)}
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
           f"--description {shlex.quote(description)} --text {shlex.quote(text)}")
    env = {"TBX_PY": pack(source), "TTS_MODEL": model}
    if os.environ.get("HF_TOKEN"):
        env["HF_TOKEN"] = os.environ["HF_TOKEN"]
    with tempfile.TemporaryDirectory(prefix="triviabix-voice-") as tmp:
        code = run_job(image, cmd, tmp, DESIGN_OUT, min_vram, max_price, max_minutes, disk, env, login,
                       boot_minutes, yes, dry_run, another, vast)
        if dry_run:
            return code
        saved = 0
        for seed in todo:
            wav = Path(tmp, f"bix-{seed}.wav")
            if not wav.is_file():
                say(f"candidate {seed} didn't come back")
                continue
            hit = store.put(SAMPLE_KIND, style, wanted[seed], wav.read_bytes(), ".wav", model=model,
                            description=description, transcript=text, seed=seed, mode=mode,
                            tags=["bix", "voice-candidate"], generator="vast.ai")
            event("made", key=hit.meta["key"], seed=seed)
            saved += 1
        say(f"saved {saved} of {len(todo)} new candidate voices in the asset store")
        return code if saved == len(todo) else (code or 1)


def voice(server: str, password: str, bix_voice: str = "ip:US_James", hours: float = 0, max_hours: float = 3,
          min_vram: float = 16, max_price: float = 0.5, disk: float = 40, image: str = TTS_IMAGE,
          login: str | None = None, boot_minutes: float = 25, yes: bool = False, dry_run: bool = False,
          another: bool = False, vast: Vast | None = None):
    """Rents a GPU running Bix's voice service, points the game server at it until the backlog is done (or
    for `hours`, to voice live lines during a game), then points it back and destroys the instance."""
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
    offer = pick_offer(vast, min_vram, max_price, disk)
    confirm(offer, minutes, yes or dry_run, "would voice lines" if dry_run else "voicing lines")
    if dry_run:
        return
    refuse_doubles(vast, another)
    on_signals()
    env = voice_env(bix_voice)
    cmd = VOICE_CMD
    if clip:
        env.update(BIX_VOICE="ip:US_James", TBX_PY=pack(CLONE_BOOT), TBX_VOICE_NAME=clip[2])
        cmd = CLONE_CMD
    with Rental(vast, offer, image, cmd, "serve",
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
        sp.add_argument("--another", action="store_true", help="rent even if a TriviaBix instance is running")

    o = sub.add_parser("offers", help="list the cheapest suitable GPUs")
    o.add_argument("--min-vram", type=float, default=16)
    o.add_argument("--max-price", type=float, default=0.5)
    o.add_argument("--disk", type=float, default=40)

    v = sub.add_parser("voice", help="make Bix's missing voice lines on a rented GPU")
    v.add_argument("--server", default=os.environ.get("TRIVIABIX_URL"), help="the game server, e.g. http://192.168.1.20:8080")
    v.add_argument("--voice", default=os.environ.get("BIX_VOICE", "ip:US_James"))
    v.add_argument("--hours", type=float, default=0, help="stay up this long for live lines (default: until done)")
    v.add_argument("--max-hours", type=float, default=3, help="hard limit (default 3)")
    v.add_argument("--image", default=TTS_IMAGE)
    v.add_argument("--boot-minutes", type=float, default=25)
    renting(v, 16, 0.5)

    g = sub.add_parser("design", help="make candidate voices for Bix on a rented GPU, kept in the asset store")
    g.add_argument("--description", required=True, help="what Bix should sound like")
    g.add_argument("--text", required=True, help="what he says in each candidate clip")
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
                  args.disk, args.image, args.login, args.boot_minutes, args.yes, args.dry_run, args.another)
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
                           args.dry_run, args.another)
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
