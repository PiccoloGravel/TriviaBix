# Generated art

Bix's cutscenes can use AI-made art: full-screen **backdrops**, cut-out **stickers**, looping
**clips** and a title card for every **category**. This folder makes them with open models on a GPU,
keeps every original so nothing is paid for twice, and ships small copies the TV can load over Wi-Fi.

Nothing here is needed to play. Scenes ask for art by tag and fall back to their drawn backgrounds and
emoji when it isn't there, so the game looks the same as before until art is made.

## How it fits together

```
art/catalog.toml ──► generate.py ──► asset store (ASSETS_DIR)          full-size originals, tagged, kept forever
content/categories.toml     │                                         (shared with Bix's voice lines, docs/assets.md)
                            └──────► web/cutscenes/assets/gen/        small WebP/MP4 copies + manifest.json for the TV
                                              │
web/cutscenes/*.json ── "art": {kind, tags} ──┘  scenes pick art by kind, tag and the room's rating
```

1. **[`catalog.toml`](catalog.toml)** lists what to make: each item has an `id`, a `prompt`, a fixed
   `seed`, `tags` and a `rating`. A shared house style goes in front of every prompt so it all looks
   like one show. Categories come from `content/categories.toml` automatically.
2. **`generate.py`** makes whatever is missing or changed. Before rendering anything it looks in the
   asset store; a picture or clip that was ever made with the same model, style and inputs is reused.
3. Every original is filed in the **asset store** (`ASSETS_DIR`, `/mnt/user/appdata/triviabix/assets`
   on Unraid) with its prompt, seed, model, tags and rating, in the layout in `docs/assets.md`.
4. A small copy goes to **`web/cutscenes/assets/gen/`** with a `manifest.json` the TV reads. Commit
   these; the Docker image ships them.
5. **Scenes** ask for art with `"art": {"kind": "sticker", "tags": ["winner"]}` (see
   [`web/cutscenes/README.md`](../web/cutscenes/README.md)), so a new scene can mix art that already
   exists without generating anything.

## Models

All open weights that allow commercial use; downloaded from Hugging Face on first use (about 45 GB).

| Job | Model | Licence | Notes |
| --- | --- | --- | --- |
| Pictures | [FLUX.1-schnell](https://huggingface.co/black-forest-labs/FLUX.1-schnell) | Apache-2.0 | 4 steps, a few seconds a picture. Not FLUX.1-dev, whose licence is non-commercial. |
| Sticker cut-outs | [BiRefNet](https://huggingface.co/ZhengPeng7/BiRefNet) | MIT | Takes the white background off stickers. |
| Clips | [Wan2.2-TI2V-5B](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B-Diffusers) | Apache-2.0 | Animates a backdrop still into a 4-second loop. Fits a 24 GB card. |

Swap a model with `ART_IMAGE_MODEL`, `ART_CUTOUT_MODEL` or `ART_VIDEO_MODEL`. The store files art by
model, so a new model makes new art next to the old instead of mixing them up.

A run does one model at a time, so each loads once: first every missing picture (FLUX draws
`ART_BATCH` of the same size per call, 8 by default or 2 on a 24 GB card, each from its own seed so it
comes out as it would alone), then every sticker's cut-out, then the clips. A batch that runs out of
memory is split in half and tried again. On a card under 40 GB, clips are decoded in tiles (a whole clip
at once needs more than 20 GB), and a clip that still runs out of memory gets one more try on a freshly
loaded model.

## Making art

**From the admin panel.** `/admin` → Rendering → Cutscene art → Check the price, then Start. It runs
`render_on_vast.py` inside the game's container. The originals go into the asset store and the TV's
copies into `ART_DIR` (`appdata/triviabix/art` on Unraid), which the server serves at
`/cutscenes/assets/gen/` in place of the art it ships with, so new art shows up on the TV without a
commit or an image rebuild.

The GPU runs the game server's own copy of `generate.py`, `library.py`, `backends/`, the catalog and the
categories, uploaded with the job, so a fix to them takes effect once the server is redeployed. The art
image only supplies PyTorch and the models' libraries; rebuild it only when `requirements.txt` or the base
image changes.

Before renting, it checks that the GPU will be able to download the models: FLUX.1-schnell is gated, so it
needs a Hugging Face token (Settings page) from an account that accepted its terms on huggingface.co. If
not, the price check says so and nothing is rented.

**On a rented GPU (vast.ai) from the command line.** Needs `gpu/vast.py` from the vast.ai runner and `VAST_API_KEY`.

```bash
python3 art/render_on_vast.py --dry-run   # what needs a GPU, which card, the price, the time limit
python3 art/render_on_vast.py             # asks before renting; --yes to skip the question
git add web/cutscenes/assets/gen && git commit -m "Add generated cutscene art"
```

It first makes everything it can from the asset store locally, for free. Only what's left goes to
the GPU, which runs [`ghcr.io/piccologravel/triviabix-art`](Dockerfile) (built by
`.github/workflows/art-image.yml`), copies the results back and is always destroyed afterwards, even
on errors or Ctrl-C. Set `ASSETS_DIR` to the Unraid share so the originals land with the rest.

What it costs, roughly, on an RTX 4090 at about $0.35 to $0.60 an hour: 15 minutes to download the
models, then a few seconds a picture and about 7 minutes a clip. The whole catalog (38 pictures and
4 clips) is about 55 minutes, so under a dollar. A re-run that changes one prompt costs a few cents.

**On your own NVIDIA GPU** (24 GB or more):

```bash
pip install -r art/requirements.txt   # plus a CUDA build of torch
python3 art/generate.py --backend gpu
```

**Without a GPU**, to try scenes out: `python3 art/generate.py --backend stub` draws labelled
placeholders. Don't commit them; a test fails if placeholder art is shipped. `--backend offline`
only uses what the asset store already has, and lists the rest in `web/cutscenes/assets/gen/.needs-gpu`.

### Useful options

- `--plan` lists what would be made. `--only trophy,clip:neon-stage` and `--kinds sticker` narrow it.
- `--force` remakes the TV's copies (originals still come from the store if they're there).
- `--prune` deletes shipped art the catalog no longer lists.
- Change only `tags` or `rating` and nothing is rendered: the manifest is just updated.
- Change a `prompt` or `seed` and only that item is made again.
- Bump `version` under `[style]` when the art direction changes, so new art is made instead of reused.

## Adding art

1. Add an entry to `catalog.toml`. Tag it with the moments it suits (the cue names, like `winner`,
   `nobody`, `roast`) and a feeling or place (`hype`, `funny`, `stage`). Rate it like a question.
2. `python3 art/render_on_vast.py`, look at it on `/cutscenes.html` (it has a gallery with every
   file's tags and prompt), and commit.
3. Use it in a scene by tag. Tags beat ids: when several pieces share a tag, a scene picks one at
   random, so it doesn't play the same way every time.

## Sizes

The TV fetches all the art in the background before it's needed, smallest first. Budgets per file:
backdrops 1280×720 WebP up to 220 KB, category cards 960×540 up to 150 KB, stickers up to 512 px
with transparency up to 120 KB, and clips 960×540 H.264 MP4 (played forwards then backwards so the
loop has no seam, no sound) up to 1.5 MB. The whole catalog is roughly 4 MB.

## Tests

`python3 -m unittest discover -s art` runs the whole pipeline with the stub backend, including the
store, reuse, offline, merging and checks that every scene's art request can be met by the catalog.
