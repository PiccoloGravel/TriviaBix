"""A stand-in for gpu/vast.py in the render tests: prints the same @tbx events without renting
anything, and writes each call to calls.log next to this folder.

voice --max-hours picks a scenario: 0.25 runs until stopped (for cancelling), 0.5 rents and then
crashes without destroying (for the server's backstop), anything else finishes at once.
"""
import json, signal, sys, time
from pathlib import Path

LOG = Path(__file__).resolve().parent.parent / "calls.log"
args = sys.argv[1:]
with LOG.open("a") as f:
    f.write(" ".join(["vast", *args]) + "\n")


def event(name, **data):
    print("@tbx " + json.dumps({"event": name, **data}), file=sys.stderr, flush=True)


def opt(name, default=None):
    return args[args.index(name) + 1] if name in args else default


def stop(*_):
    print("[vast] stopped", file=sys.stderr, flush=True)
    event("destroyed", instance=42, seconds=3, cost=0.0003)
    sys.exit(130)


cmd = args[0]
if cmd in ("cleanup", "destroy"):
    print(f"[vast] {cmd} done", file=sys.stderr)
    sys.exit(0)
if cmd == "voice":
    hours = float(opt("--max-hours", "3"))
    event("backlog", lines=12)
    event("offer", gpu="RTX 4090", vram_gb=24, price_hr=0.3, location="Norway", minutes=hours * 60,
          max_cost=round(0.3 * hours, 2))
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
