"""Voices a whole backlog of Bix's lines in one go, on the GPU it runs on.

    python batch.py --in /in --out /out [--workers auto] [--stop-at EPOCH]

/in/lines.jsonl has one {"key": ..., "text": ...} per line. Each line's audio is written to
/out/<key>.<ext> as soon as it's made, and /out/voice.json says which voice and format they're in, so
whatever is done when the run stops is still usable. gpu/vast.py uploads the backlog, runs this, pulls
the audio back while it works and files it in the game server's asset store.

The model is small (0.5B) and makes one line at a time, which leaves most of a GPU idle, so this loads
several copies of it in separate processes (--workers, by default as many as fit in GPU memory) and
hands each the next line as it finishes. Longest lines go first, so no copy is left with a long one at
the end.

It only needs the image's server.py (Voice, the backends and encode), so a running image doesn't have to
be rebuilt to use it: gpu/vast.py sends this file along.
"""

from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import os
import queue
import subprocess
import sys
import time

# server.py is next to this file in the repo; in the image (where this runs from gpu/vast.py) it's in /app.
sys.path.insert(0, os.environ.get("TTS_APP") or (os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals()
                                                 else "/app"))
import server  # noqa: E402

# GPU memory one copy of the model takes while it works, with room to spare, in GB.
PER_WORKER_GB = 4.0
MAX_WORKERS = 6
# How long to wait on the last lines after the queue is empty and nothing has come back.
STUCK_S = 600


def gpu_gb() -> float:
    """The GPU's memory, asked of nvidia-smi so this process never touches CUDA (the workers are forked)."""
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits"],
                             capture_output=True, text=True, timeout=20).stdout
        return float(out.split()[0]) / 1024
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return 0.0


def worker_count(asked: str, backend: str) -> int:
    if asked != "auto":
        return max(1, min(16, int(asked)))
    if backend == "stub":
        return 4
    gb = gpu_gb()
    return max(1, min(MAX_WORKERS, int((gb - 2) // PER_WORKER_GB))) if gb else 2


def work(n: int, spec: str, backend: str, fmt: str, out: str, jobs, results):
    """One copy of the model: makes lines from `jobs` until it gets None."""
    try:
        voice = server.Voice(spec)
        model = server.StubBackend(voice) if backend == "stub" else server.MingBackend(voice)
    except BaseException as e:  # noqa: BLE001 (report anything, even SystemExit, then stop)
        results.put(("dead", n, f"{type(e).__name__}: {e}", 0.0))
        return
    results.put(("ready", n, "", 0.0))
    ext = fmt
    while True:
        job = jobs.get()
        if job is None:
            return
        key, text = job
        started = time.monotonic()
        try:
            audio = server.encode(model.synthesize(text), model.sample_rate, fmt)
            path = os.path.join(out, f"{key}.{ext}")
            with open(path + ".part", "wb") as f:
                f.write(audio)
            os.replace(path + ".part", path)
            results.put(("made", n, key, time.monotonic() - started))
        except Exception as e:  # one bad line shouldn't stop the run
            results.put(("failed", n, f"{key}: {type(e).__name__}: {e}", time.monotonic() - started))


def load_lines(folder: str) -> list[tuple[str, str]]:
    lines, seen = [], set()
    with open(os.path.join(folder, "lines.jsonl"), encoding="utf-8") as f:
        for row in f:
            if not row.strip():
                continue
            item = json.loads(row)
            key, text = str(item["key"]), str(item["text"]).strip()
            if key in seen or not text or len(text) > server.MAX_TEXT or not key.isalnum():
                continue
            seen.add(key)
            lines.append((key, text))
    lines.sort(key=lambda kt: -len(kt[1]))
    return lines


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--in", dest="inp", default="/in")
    ap.add_argument("--out", default="/out")
    ap.add_argument("--workers", default=os.environ.get("TTS_WORKERS", "auto"))
    ap.add_argument("--stop-at", type=float, default=0, help="stop by this time (epoch seconds), keeping what's made")
    args = ap.parse_args(argv)

    spec = os.environ.get("BIX_VOICE", "") or "ip:US_James"
    voice = server.Voice(spec)
    backend = os.environ.get("TTS_BACKEND", "ming").lower()
    fmt = os.environ.get("TTS_FORMAT", "ogg").lower()
    if backend == "stub":
        fmt = "wav"  # the stub has no soundfile to encode with
    model = os.environ.get("TTS_MODEL", "inclusionAI/Ming-3.0-tts-0.5b") if backend != "stub" else "stub"
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "voice.json"), "w") as f:
        json.dump({"voice": voice.key, "format": fmt, "content_type": server.CONTENT_TYPES[fmt], "model": model}, f)

    lines = load_lines(args.inp)
    todo = [kt for kt in lines if not os.path.exists(os.path.join(args.out, f"{kt[0]}.{fmt}"))]
    total = len(todo)
    count = min(worker_count(args.workers, backend), max(1, total))
    print(f"[batch] {total} lines in {voice.key}, up to {count} copies of the model at once", flush=True)
    if not total:
        return 0

    # Forked, so this process must not start CUDA itself; each worker loads its own copy of the model.
    ctx = mp.get_context("fork")
    jobs, results = ctx.Queue(), ctx.Queue()
    for kt in todo:
        jobs.put(kt)
    procs = {}

    def start(n):
        p = ctx.Process(target=work, args=(n, spec, backend, fmt, args.out, jobs, results), daemon=True)
        p.start()
        procs[n] = p

    # The first copy loads alone (the first start downloads the model), then the rest join it.
    start(0)
    made = failed = ready = dead = 0
    busy_s = 0.0
    began = time.monotonic()
    first_at = None
    last_note = 0.0
    last_result = time.monotonic()
    stopped = None
    while made + failed < total:
        if args.stop_at and time.time() > args.stop_at:
            stopped = "the time limit is near"
            break
        try:
            what, n, info, secs = results.get(timeout=5)
        except queue.Empty:
            if all(not p.is_alive() for p in procs.values()):
                stopped = "every worker stopped"
                break
            # A copy that crashed mid-line takes that line with it; don't wait for it forever.
            if ready and jobs.empty() and time.monotonic() - last_result > STUCK_S:
                stopped = "the last lines never came back"
                break
            continue
        last_result = time.monotonic()
        if what == "ready":
            ready += 1
            if ready == 1:
                first_at = time.monotonic()
                print(f"[batch] model loaded in {first_at - began:.0f}s", flush=True)
                for m in range(1, count):
                    start(m)
            else:
                print(f"[batch] {ready} copies working", flush=True)
        elif what == "dead":
            dead += 1
            print(f"[batch] copy {n} couldn't load: {info}", flush=True)
            if ready == 0 and n == 0:
                return 1
        elif what == "made":
            made += 1
            busy_s += secs
        elif what == "failed":
            failed += 1
            print(f"[batch] failed {info}", flush=True)
        now = time.monotonic()
        if now - last_note > 15 or made + failed == total:
            last_note = now
            mins = (now - (first_at or began)) / 60
            rate = made / mins if mins > 0 else 0
            left = total - made - failed
            eta = f", about {left / rate:.0f} min to go" if rate and left else ""
            print(f"[batch] voiced {made} of {total} ({rate:.0f} lines/min on {ready} copies{eta})", flush=True)
    for _ in procs:
        jobs.put(None)
    for p in procs.values():
        p.join(timeout=10)
        if p.is_alive():
            p.kill()
    secs = time.monotonic() - (first_at or began)
    with open(os.path.join(args.out, "stats.json"), "w") as f:
        json.dump({"made": made, "failed": failed, "total": total, "workers": ready, "seconds": round(secs, 1),
                   "load_seconds": round((first_at or began) - began, 1), "busy_seconds": round(busy_s, 1)}, f)
    print(f"[batch] done: {made} made, {failed} failed of {total} in {secs / 60:.1f} min"
          + (f"; stopped because {stopped}" if stopped else ""), flush=True)
    return 0 if made == total else 1


if __name__ == "__main__":
    sys.exit(main())
