# Generated assets

Everything TriviaBix generates (Bix's spoken lines, images, video clips) is kept in one store so it is
made once and reused forever, whichever machine or rented GPU made it. Before generating anything, look
it up; if it's there, use it.

The store is a plain folder: `ASSETS_DIR`, `/data/assets` in Docker, which is
`/mnt/user/appdata/triviabix/assets` on Unraid. Back it up with appdata. `gpu/assets.py` reads and writes
it from Python; `server/src/voice.rs` does the same for voice lines.

## Layout

```
assets/
  voice/                     kind: voice, image, clip (add more as needed)
    current-voice.txt        voice only: the style the cache was last filled in
    3f9a1c2b7d4e/            style folder: sha256(style)[:12]
      style.txt              the style string, e.g. "ip:US_James" or "flux.1-schnell:bix-cartoon-v1"
      9c1e...e2.ogg          the asset: <key>.<ext>
      9c1e...e2.json         its metadata
```

- **Style** is a string naming what makes the same inputs come out differently: the voice for speech
  (whatever the TTS service sends as `X-Voice`), or the model plus a named look for images and clips.
  Change the style (a new voice, a new art direction) and everything is made fresh in a new folder, while
  the old folder stays for going back.
- **Key** is `sha256(canonical_json(inputs))[:32]` in hex. `inputs` is a JSON object holding everything
  else that changes the output: `{"text": "..."}` for a voice line, `{"prompt", "width", "height",
  "seed", ...}` for an image. Canonical JSON is sorted keys, no spaces (`","` and `":"`), UTF-8 as is
  (Python: `json.dumps(x, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`). Fix the seed for
  anything meant to be reused, or the key won't match the next time.
- A file is only there once it's complete: write `<key>.<ext>.part`, then rename. Write the asset first
  and its `.json` second; an asset without metadata is still valid.

## Metadata

```json
{
  "kind": "image",
  "style": "flux.1-schnell:bix-cartoon-v1",
  "inputs": {"prompt": "Bix the robot host bowing", "width": 1024, "height": 576, "seed": 7},
  "key": "9c1e...e2",
  "file": "9c1e...e2.png",
  "content_type": "image/png",
  "bytes": 482113,
  "model": "black-forest-labs/FLUX.1-schnell",
  "created": 1793001234,
  "generator": "vast.ai RTX 4090",
  "rating": "PG",
  "tags": ["cutscene", "bix"]
}
```

`created` is Unix seconds. `rating` (PG, PG-13, MA, NC-17) and `tags` are optional; set them when
known so the game can pick assets that suit a room's maximum rating. Unknown fields are kept, so a job
can add its own.

## Voice lines

`style` is the TTS voice, `inputs` is `{"text": line}`, and the file is `.ogg` (or `.flac`, `.wav`). The
game server serves them at `/api/voice/<key>`, makes missing ones through the TTS service (on the
Unraid GPU, a rented one through `gpu/vast.py voice`, or none), and plays whatever is cached when no
service is up.

## Candidate voices

`gpu/vast.py design` keeps the voices it designs for Bix as kind `voice-sample`. `style` is
`ming-<mode>:<model>`, `inputs` is `{"description", "text", "seed"}`, and the file is a `.wav` whose metadata
also has `description`, `transcript` and `seed`. `BIX_VOICE=clone:<key>` makes one Bix's voice: a rented GPU
gets the clip and transcript and clones it for every line.
