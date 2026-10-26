"""Rent a vast.ai GPU for TriviaBix's generation work, and always give it back. Python 3.9+, stdlib only.

    export VAST_API_KEY=...            # from https://cloud.vast.ai/manage-keys/ ; never put it in the repo
    python3 gpu/vast.py offers                                   # what you'd get, renting nothing
    python3 gpu/vast.py voice --server http://192.168.1.20:8080  # make Bix's missing lines, then stop
    python3 gpu/vast.py voice --server ... --hours 4             # also voice live lines for a game night
    python3 gpu/vast.py run --image IMAGE --cmd "python generate.py --out /out" --out /out --to ./art
    python3 gpu/vast.py list                                     # TriviaBix instances you're paying for
    python3 gpu/vast.py cleanup                                  # destroy all of them

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
import secrets
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

API = os.environ.get("VAST_URL", "https://console.vast.ai").rstrip("/")
LABEL = "triviabix"
TTS_IMAGE = "ghcr.io/piccologravel/triviabix-tts:latest"
PORT = 8000
# How the voice image starts its service, with the rental's token so only the game server can use it.
VOICE_CMD = "cd /app && TTS_TOKEN=$TBX_TOKEN exec python server.py"


class Abort(Exception):
    """Something the person should read, without a traceback."""


class Gone(Abort):
    """vast.ai has no such thing (404)."""


def say(*args):
    print("[vast]", *args, file=sys.stderr, flush=True)


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


def boot_args() -> list:
    packed = base64.b64encode(zlib.compress(WRAPPER.encode(), 9)).decode()
    return ["python3", "-c", f"import base64,zlib;exec(zlib.decompress(base64.b64decode('{packed}')))"]


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
        self.body = {
            "client_id": "me", "image": image, "disk": disk, "label": f"{LABEL}-{kind}",
            "runtype": "args", "args": boot_args(), "image_login": login,
            "env": {f"-p {port}:{port}": "1", **(env or {}), "TBX_MODE": mode, "TBX_CMD": cmd,
                    "TBX_OUT": out, "TBX_TOKEN": self.token, "TBX_PORT": str(port),
                    "TBX_DEADLINE": str(int(self.deadline))},
        }

    def __enter__(self):
        self.id = self.vast.create(self.offer["id"], self.body)
        say(f"rented instance {self.id}: {describe(self.offer)}")
        return self

    def __exit__(self, *exc):
        if self.id is not None:
            self.vast.destroy(self.id)

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


def voice(server: str, password: str, bix_voice: str = "ip:US_James", hours: float = 0, max_hours: float = 3,
          min_vram: float = 16, max_price: float = 0.5, disk: float = 40, image: str = TTS_IMAGE,
          login: str | None = None, boot_minutes: float = 25, yes: bool = False, dry_run: bool = False,
          another: bool = False, vast: Vast | None = None):
    """Rents a GPU running Bix's voice service, points the game server at it until the backlog is done (or
    for `hours`, to voice live lines during a game), then points it back and destroys the instance."""
    if bix_voice.startswith("clone:"):
        raise Abort("a cloned voice needs its clip on the GPU, which isn't supported yet; use an ip: voice")
    vast = vast or Vast()
    game = Game(server, password)
    backlog = game.status()["backlog"]
    say(f"{backlog} lines waiting to be made")
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
    env = {"BIX_VOICE": bix_voice, "PORT": str(PORT)}
    with Rental(vast, offer, image, VOICE_CMD, "serve",
                minutes, env=env, disk=disk, login=login, kind="voice") as r:
        base = r.address(boot_minutes)
        say(f"voice service at {base}; loading the model (the first start downloads about 3 GB)")
        ready_by = time.time() + boot_minutes * 60
        while True:
            try:
                health = json.load(urllib.request.urlopen(f"{base}/healthz", timeout=10))
                if health.get("ready"):
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
    sub = p.add_subparsers(dest="cmd", required=True)

    def renting(sp, vram, price):
        sp.add_argument("--min-vram", type=float, default=vram, help=f"GB of GPU memory (default {vram:g})")
        sp.add_argument("--max-price", type=float, default=price, help=f"$/hour, all in (default {price:g})")
        sp.add_argument("--disk", type=float, default=40, help="GB of disk (default 40)")
        sp.add_argument("--login", default=os.environ.get("VAST_IMAGE_LOGIN"),
                        help='for a private image: "-u USER -p TOKEN ghcr.io" (or VAST_IMAGE_LOGIN)')
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
    args = p.parse_args(argv)

    try:
        if args.cmd == "offers":
            for offer in Vast().offers(args.min_vram, args.max_price, args.disk):
                print(describe(offer))
        elif args.cmd == "voice":
            if not args.server:
                raise Abort("pass --server with the game server's address (or set TRIVIABIX_URL)")
            password = os.environ.get("ADMIN_PASSWORD")
            if not password:
                raise Abort("set ADMIN_PASSWORD to the game server's admin password")
            voice(args.server, password, args.voice, args.hours, args.max_hours, args.min_vram, args.max_price,
                  args.disk, args.image, args.login, args.boot_minutes, args.yes, args.dry_run, args.another)
        elif args.cmd == "run":
            env = dict(e.split("=", 1) for e in args.env)
            return run_job(args.image, args.cmd, args.to, args.out, args.min_vram, args.max_price,
                           args.max_minutes, args.disk, env, args.login, args.boot_minutes, args.yes,
                           args.dry_run, args.another)
        elif args.cmd == "list":
            for i in Vast().ours():
                up = (time.time() - (i.get("start_date") or time.time())) / 3600
                print(f"{i['id']}  {i.get('label')}  {i.get('actual_status')}  {i.get('gpu_name')}  "
                      f"${i.get('dph_total', 0):.3f}/hr  up {up:.1f}h")
        elif args.cmd == "cleanup":
            vast = Vast()
            ours = vast.ours()
            for i in ours:
                vast.destroy(i["id"])
            say(f"destroyed {len(ours)} instance(s)")
        return 0
    except Abort as e:
        say(str(e))
        return 1
    except KeyboardInterrupt:
        say("stopped")
        return 130


if __name__ == "__main__":
    sys.exit(main())
