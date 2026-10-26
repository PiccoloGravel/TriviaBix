"""A stand-in for gpu/vast.py in the render tests: prints the same @tbx events without renting
anything, and writes each call to calls.log next to this folder.

design writes --count candidate voices into the asset store (--library), skipping ones there.
voice --max-hours picks a scenario: 0.25 runs until stopped (for cancelling), 0.5 rents and then
crashes without destroying (for the server's backstop), anything else finishes at once.
"""
import hashlib, json, os, signal, sys, time
from pathlib import Path

LOG = Path(__file__).resolve().parent.parent / "calls.log"
args = sys.argv[1:]
with LOG.open("a") as f:
    # Which key it was given, so the tests can tell the Settings page's key reached it.
    key = os.environ.get("VAST_API_KEY")
    f.write(" ".join(["vast", *args]) + (f" [key {key}]" if key else "") + "\n")


def event(name, **data):
    print("@tbx " + json.dumps({"event": name, **data}), file=sys.stderr, flush=True)


def opt(name, default=None):
    return args[args.index(name) + 1] if name in args else default


def stop(*_):
    print("[vast] stopped", file=sys.stderr, flush=True)
    event("destroyed", instance=42, seconds=3, cost=0.0003)
    sys.exit(130)


cmd = args[0]
if cmd == "design":
    folder = Path(opt("--library"), "voice-sample", "0123456789ab")
    texts = [args[i + 1] for i, a in enumerate(args) if a == "--text"]
    keys = [hashlib.sha256(f"{n}|{t}".encode()).hexdigest()[:32] for n, t in enumerate(texts, 1)]
    todo = [k for k in keys if not (folder / f"{k}.wav").exists()]
    event("todo", count=len(todo), total=len(keys), minutes=40)
    if not todo:
        sys.exit(0)
    event("offer", gpu="RTX 3090", vram_gb=24, price_hr=0.2, location="Norway", minutes=40, max_cost=0.13)
    if "--dry-run" in args:
        sys.exit(0)
    event("rented", instance=43, price_hr=0.2)
    folder.mkdir(parents=True, exist_ok=True)
    for n, key in enumerate(todo, 1):
        (folder / f"{key}.wav").write_bytes(b"RIFF" + key.encode())
        (folder / f"{key}.json").write_text(json.dumps({
            "kind": "voice-sample", "key": key, "file": f"{key}.wav", "created": 1793000000 + n, "seed": n,
            "description": opt("--description"), "transcript": texts[n - 1],
            "inputs": {"description": opt("--description"), "text": texts[n - 1], "seed": n}}))
        event("made", key=key, seed=n)
    event("destroyed", instance=43, seconds=600, cost=0.033)
    sys.exit(0)
if cmd in ("cleanup", "destroy"):
    print(f"[vast] {cmd} done", file=sys.stderr)
    sys.exit(0)
if cmd == "voice":
    hours = float(opt("--max-hours", "3"))
    event("backlog", lines=12)
    event("offer", gpu="RTX 4090", vram_gb=24, price_hr=0.3, location="Norway", minutes=hours * 60,
          max_cost=round(0.3 * hours, 2), est_minutes=10.2, est_cost=0.05)
    if "--dry-run" in args:
        print("[vast] would voice lines on RTX 4090", file=sys.stderr)
        sys.exit(0)
    signal.signal(signal.SIGTERM, stop)
    event("rented", instance=42, price_hr=0.3)
    if hours == 0.5:
        print("[vast] something broke", file=sys.stderr, flush=True)
        sys.exit(1)
    steps = 600 if hours == 0.25 else 3
    for n in range(steps):
        event("backlog", lines=max(0, 12 - 4 * (n + 1)))
        print(f"[vast] {max(0, 12 - 4 * (n + 1))} lines to go", file=sys.stderr, flush=True)
        time.sleep(0.05)
    event("destroyed", instance=42, seconds=60, cost=0.005)
    sys.exit(0)
sys.exit(2)
