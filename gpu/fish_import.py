#!/usr/bin/env python3
"""Files Bix's lines voiced by Fish Audio Drama 3 in the game's asset store, as the voice pack fish:bix.

Fish runs on the same rented GPU as CouchCritters' story lines (couchcritters/tools/voice/render.js --extra),
which leaves one <line hash>.wav per line, named by GET /api/admin/voice/backlog?voice=fish:bix. This takes
that folder, evens the loudness, makes each an Ogg Opus clip and uploads it through the game server's admin
API, then makes fish:bix Bix's voice (BIX_VOICE), so every game plays them. A line with no clip, such as one
with a player's name in it, is read by the TV's browser voice as before.

    export ADMIN_PASSWORD=<the game's admin password>
    python3 gpu/fish_import.py --server http://localhost:8080 ~/couchcritters/voice/runs/<run>/out/lines
    python3 gpu/fish_import.py ... --dry-run     # count what would go up, upload nothing

Lines already filed are skipped, so it can be run again on the same folder, or on a later run's.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from vast import Abort, Game, say  # noqa: E402

VOICE = "fish:bix"
MODEL = "fishaudio/drama-3"
HASH_LEN = 32


def is_hash(name: str) -> bool:
    return len(name) == HASH_LEN and all(c in "0123456789abcdef" for c in name)


def encode(wav: Path, tmp: Path) -> tuple[bytes, str]:
    """The clip as the TV gets it: loudness evened, Opus 32 kbps mono. Without ffmpeg, the .wav itself."""
    if not shutil.which("ffmpeg"):
        return wav.read_bytes(), "audio/wav"
    out = tmp / (wav.stem + ".ogg")
    r = subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(wav),
                        "-af", "loudnorm=I=-18:TP=-1.5:LRA=11", "-ar", "24000", "-ac", "1",
                        "-c:a", "libopus", "-b:a", "32k", str(out)], capture_output=True, text=True)
    if r.returncode:
        raise Abort(f"ffmpeg failed on {wav.name}: {r.stderr.strip()}")
    return out.read_bytes(), "audio/ogg"


def run(server: str, password: str, folder: Path, dry_run: bool = False, switch: bool = True) -> int:
    if not folder.is_dir():
        raise Abort(f"{folder} isn't a folder")
    wavs = sorted(p for p in folder.glob("*.wav") if is_hash(p.stem))
    game = Game(server, password)
    # What the game still lacks in this voice; a clip for anything else is old or already filed.
    waiting = {line["key"] for line in game.backlog(VOICE)}
    todo = [w for w in wavs if w.stem in waiting]
    say(f"{len(wavs)} clips in {folder}, {len(todo)} of them still needed ({len(waiting)} lines waiting in {VOICE})")
    if dry_run:
        return 0
    done = failed = 0
    with tempfile.TemporaryDirectory(prefix="triviabix-fish-") as tmp:
        for w in todo:
            audio, kind = encode(w, Path(tmp))
            try:
                game.put_line(w.stem, audio, kind, VOICE, MODEL)
                done += 1
            except urllib.error.HTTPError as e:
                # 404: the line was edited since the export, so the game no longer has it.
                say(f"{w.stem}: the game turned it down ({e.code})")
                failed += 1
            if done and done % 500 == 0:
                say(f"{done} of {len(todo)} filed")
    say(f"filed {done} clips" + (f", {failed} turned down" if failed else ""))
    if switch and wavs:
        game.call("PUT", "/api/admin/settings", {"key": "BIX_VOICE", "value": VOICE})
        say(f"Bix now speaks in {VOICE}; {len(waiting) - done} lines still use the browser voice")
    return 1 if failed and not done else 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("folder", type=Path, help="the render run's folder of <line hash>.wav files")
    p.add_argument("--server", default=os.environ.get("TRIVIABIX_SERVER", "http://localhost:8080"))
    p.add_argument("--dry-run", action="store_true", help="count what would go up, upload nothing")
    p.add_argument("--keep-voice", action="store_true", help="file the clips but leave BIX_VOICE as it is")
    a = p.parse_args(argv)
    password = os.environ.get("ADMIN_PASSWORD")
    if not password:
        say("set ADMIN_PASSWORD to the game's admin password")
        return 2
    try:
        return run(a.server, password, a.folder.expanduser(), a.dry_run, not a.keep_voice)
    except Abort as e:
        say(str(e))
        return 1


if __name__ == "__main__":
    sys.exit(main())
