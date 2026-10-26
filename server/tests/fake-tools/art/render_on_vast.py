"""A stand-in for art/render_on_vast.py in the render tests."""
import json, sys
from pathlib import Path

args = sys.argv[1:]
(Path(__file__).resolve().parent.parent / "calls.log").open("a").write(" ".join(["art", *args]) + "\n")


def event(name, **data):
    print("@tbx " + json.dumps({"event": name, **data}), file=sys.stderr, flush=True)


out = Path(args[args.index("--out") + 1])
out.mkdir(parents=True, exist_ok=True)
event("todo", count=3, clips=1, minutes=23, limit=34)
event("offer", gpu="RTX 4090", vram_gb=24, price_hr=0.4, location="Norway", minutes=34, max_cost=0.23)
if "--dry-run" in args:
    sys.exit(0)
event("rented", instance=7, price_hr=0.4)
(out / "manifest.json").write_text(json.dumps({"files": {"bg/test.webp": {"kind": "backdrop"}}}))
(out / "bg").mkdir(exist_ok=True)
(out / "bg" / "test.webp").write_bytes(b"RIFF fake")
event("destroyed", instance=7, seconds=600, cost=0.067)
