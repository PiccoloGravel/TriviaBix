# Rented GPUs (vast.ai)

The Unraid box doesn't need a GPU. When there's something to generate, `vast.py` rents one on
[vast.ai](https://vast.ai), does the work, saves the results at home, and destroys the instance. A 24 GB
card such as an RTX 4090 is usually $0.25 to $0.45 an hour, so making all of Bix's lines once costs cents.
Everything made is kept in the asset store ([docs/assets.md](../docs/assets.md)) and never made twice.

The easiest way to use it is the admin panel: **`/admin` → Rendering** runs these same commands inside the
game's container, with a price check first, a confirm, a live log, a cancel button and a list of past jobs
and what they cost (see [deploy/unraid/README.md](../deploy/unraid/README.md#rendering-from-the-admin-panel)).
The game server just needs `VAST_API_KEY` set.

It's one Python 3 file with no dependencies, so it also runs anywhere else: a laptop, or a throwaway
container on Unraid (below).

## Setting up, once

1. Make a vast.ai account and **add a small amount of credit, such as $5 to $10**. vast.ai is prepaid, so
   that credit is the hard limit on what can ever be spent. Leave auto-reload off.
2. Make an API key at <https://cloud.vast.ai/manage-keys/>. Keep it out of the repo and out of chat; set it
   as `VAST_API_KEY` where you run the script (on Unraid, in the User Scripts script or the container's
   variables).
3. vast.ai has to be able to pull the voice and art images. If they're private on GHCR, put a classic
   GitHub token with only `read:packages` in the admin panel: **Settings → Private images (GitHub)**, then
   press **Test**. From the command line, set `GHCR_TOKEN` (and `GHCR_USER` if it isn't PiccoloGravel), or
   pass `--login "-u USER -p TOKEN ghcr.io"`. See [deploy/unraid](../deploy/unraid/README.md#1-let-unraid-pull-the-image-once).

## Bix's voice

```sh
export VAST_API_KEY=...  ADMIN_PASSWORD=<the game's admin password>
python3 gpu/vast.py voice --server http://192.168.1.20:8080 --dry-run   # shows the GPU and the price
python3 gpu/vast.py voice --server http://192.168.1.20:8080             # does it
```

It logs in to the game server's admin API and gets every line still waiting for audio in Bix's voice
(renting nothing if there are none). It rents the GPU expected to finish the job for the least money,
starts the `triviabix-tts` image there and uploads the whole backlog in one go. On the GPU,
`tts/batch.py` (sent along, so the image needn't be rebuilt) loads a few copies of the model, as many as
fit in its memory (about 3.5 GB each, so four on a 16 GB card), and each voices 24 lines side by side in every model call (Ming's own sampler does
one line at a time, which leaves the GPU mostly idle), longest lines first. Every minute
the audio made so far comes back and is filed in `appdata/triviabix/assets/voice` through the game
server, so a run that's stopped keeps what it made. When it's done the instance is destroyed. Cached
lines keep playing with no GPU at all; only new lines with player names fall back to the browser voice.

Each run measures how many lines a minute it made and keeps that in the asset store
(`gpu-speeds.json`), so the next price check's "expected time and cost" is based on real numbers.
Until the first run it guesses 150 lines a minute on an RTX 4090.

`--hours N` instead keeps a voice service running on the GPU for N hours with the game server pointed
at it, so live lines (new players' names) are in Bix's voice during a game night.

### Picking the GPU

Only GPUs the images' PyTorch can run are rented. The voice image (PyTorch 2.6, CUDA 12.4) runs Ampere to
Hopper (compute 8.0 to 9.0); RTX 50-series and other Blackwell cards are left out until it moves to a newer
PyTorch, then set `VAST_MAX_COMPUTE_CAP=1200`. The art image (PyTorch 2.7, CUDA 12.8) runs Blackwell too, so
art runs rent up to compute 12.0, on hosts whose driver runs CUDA 12.8 (`VAST_MIN_CUDA`). On the GPU, a card
counts as supported when the build has kernels for its major version at an equal or lower minor (an sm_86
build runs an RTX 40-series, sm_89) or PTX for it; anything else gets a small test calculation first. If a
GPU still fails with a CUDA error, the run stops at once instead of failing every line.

Offers under `--max-price` are ordered by what the whole job is expected to cost, from a table of how
fast each GPU model is next to an RTX 4090 (`SPEED` in vast.py): a card that costs twice as much an hour
but finishes three times sooner wins. Cards that wouldn't finish within the time limit go last. The art
job does the same, counting cards under 40 GB as slower, since they have to move the models in and out.

### What gets made ahead

Everything the game can know in advance, so a game night needs no GPU at all:

- every fixed line, every question, and each question's own lead-in and reveal lines;
- the host's lines with each category, round number and difficulty filled in;
- the answer reveals ("It was Paris!") for every answer;
- the lines about players (joining, fastest, streaks, last place, winning) for the 12 names that have
  played the most, plus any in `VOICE_NAMES` (set it on the game server, e.g. `VOICE_NAMES=JDog,Sam,Alex`,
  so they're ready before the first game).

During a game, Bix picks from the lines that already have audio, so nearly everything he says is in his
voice. Only lines that can't be known ahead (a newcomer's name, a score, someone's wrong answer) use the
browser voice. With the starter questions that's roughly 6,000 lines; the first run takes a couple of
hours and about $1, and after that only new questions, edits and new regulars are made. If a run hits
`--max-hours`, the next one carries on where it stopped. Run it again after a few game nights so new
regulars get their lines.

Live lines are possible too (`--hours 3` keeps the GPU up for a game), but they travel over the internet
and aren't needed for a good night.

The first start takes about 10 minutes (image pull and a 3 GB model download); after that it's quick on
the same machine type. Live lines travel over the internet, so a slow one falls back to the browser voice
after 3.5 s as usual.

`--voice` (BIX_VOICE) is a built-in voice (`ip:UK_George` and so on) or a cloned one: `clone:<key>` for a
voice designed below, or `clone:/path/bix.wav` for a clip on this machine with its transcript in `bix.txt`.
For a cloned voice the GPU starts with a small receiver instead of the voice service; `vast.py` sends it
the clip and transcript (behind the rental's token), and it then starts the service speaking in that voice.

## Designing Bix's voice

```sh
python3 gpu/vast.py design --description "A washed-up game show host..." --text "Welcome back..." --dry-run
python3 gpu/vast.py design --description "..." --text "First line..." --text "Second line..." --count 4
```

Rents a GPU running the voice image, makes `--count` candidate clips with `tts/make_voice.py` (sent along
with the job, so the image doesn't need rebuilding when it changes), and keeps them in the asset store as
kind `voice-sample` (see [docs/assets.md](../docs/assets.md)). Candidate N says the Nth `--text`, so each one
sounds different; the Pipeline page picks them at random from an editable list. The same description, line and take number
always give the same clip, so candidates already in the store are reused and only new ones are paid for.
The admin panel's Pipeline page runs this, plays the candidates and sets `BIX_VOICE=clone:<key>` for the
one picked. Defaults: 40 minutes at most, `--max-price` 0.50 (the Settings page's voice limit).

## Any other job (images, video)

```sh
python3 gpu/vast.py run --image ghcr.io/piccologravel/triviabix-art:latest \
  --cmd "python generate.py --out /out" --out /out --to ./art-output --min-vram 24 --max-minutes 45
```

It runs the command in the image, tars `/out` when it exits, downloads it into `--to`, and destroys the
instance. The exit code is the job's. The image needs `python3` and no `ENTRYPOINT` (the command is
started with `python3 -c`). From Python: `from vast import run_job`. Add the results to the asset store
with `python3 gpu/assets.py import ./art-output --kind image --style <style>`.

## When a GPU is taken or can't start

Offers come and go fast. If the GPU that was priced is rented by someone else before the job rents it,
`vast.py` rents the next cheapest one that matches, never above the price limit already confirmed (with
`--max-price`, or the Settings page's limit for jobs from the admin panel). If every one is taken, nothing is
rented or charged. If the GPU can't download the image (a private image without a login, say), the job
stops at once with the reason, instead of waiting out the boot time, and the instance is destroyed.

Whenever a rented instance fails (it stops while starting, its job crashes, it runs out of time), `vast.py`
first fetches the last lines the instance logged from vast.ai and prints them, prefixed `[gpu]`, so the
job log on the Pipeline page says why before the instance is destroyed.

The job reaches the instance in a single env value, `TBX_JOB` (base64 of the command, its env and the
small wrapper that runs it), and the boot command has no spaces or quotes, so neither gets mangled on
the way into the container.

## Limits on spending

- **Price:** `--max-price` is the most per hour, all in (default $0.50). Only verified, reliable machines
  with a single Ampere-or-newer GPU and fast internet are considered, cheapest first.
- **Time:** `--max-hours` (voice, default 3) and `--max-minutes` (jobs, default 60). Before renting, it
  prints the most the run can cost and asks; pass `--yes` to skip the question in scripts.
- **Always destroyed:** when the work ends, fails, hits the time limit, or you press Ctrl-C. As a backstop
  the instance also destroys itself at the time limit, so it stops even if the computer running the
  script goes to sleep or loses its connection. If it can't, it exits, which stops the GPU charge; a
  stopped instance's disk costs pennies a day until `cleanup`.
- **Unreachable GPUs are given back:** if a job's address doesn't answer for 5 minutes after vast.ai says
  the instance is running (`VAST_UNREACHABLE_MINUTES`), it's destroyed. If it never answered at all, the
  next GPU on a different machine is tried (up to 3); if it answered and then went quiet, the job fails.
  The wait is logged about once a minute.
- **Stuck starts are given back:** what vast.ai says while an instance starts (its image pull progress)
  goes in the log. An instance whose status doesn't change for 5 minutes (`VAST_STALL_MINUTES`), or that
  isn't running within the boot limit (`--boot-minutes`: 12 for jobs, 10 for art, 15 for voice), is
  destroyed and the next GPU on a different machine is tried, up to 3 in all.
- **One of each kind at a time:** it won't rent a second instance for the same kind of job (voice, design,
  or art with `run --kind art`) while one is running (`--another` to override). Different kinds, like
  voice and art, run side by side, each on its own GPU.
- `python3 gpu/vast.py list` shows what's running and its price; `python3 gpu/vast.py cleanup` destroys
  every TriviaBix instance, and `python3 gpu/vast.py destroy ID` one of them. Instances are labeled `triviabix-...` and nothing else on the account is
  touched.

## From Unraid

Save this repo's `gpu/` folder to `/mnt/user/appdata/triviabix/gpu`, then from the Unraid terminal or a
User Scripts script:

```sh
docker run --rm -it -e VAST_API_KEY=... -e ADMIN_PASSWORD=... \
  -v /mnt/user/appdata/triviabix/gpu:/gpu python:3.12-alpine \
  python /gpu/vast.py voice --server http://<unraid-ip>:8080
```

Scheduled after adding questions, say nightly with `--yes`, it rents nothing on the nights there's nothing
new to make.

## For the admin panel

With `TBX_EVENTS=1`, `vast.py` (and `art/render_on_vast.py`) also print lines like
`@tbx {"event": "offer", "gpu": "RTX 4090", "price_hr": 0.31, "max_cost": 0.93, ...}` to stderr: `offer`
before renting, `rented` and `destroyed` (with the cost) around the instance, `backlog` while voicing and
`todo` for the art and voice designs, and `made` for each new candidate voice. The game server (`server/src/render.rs`) reads them to show prices, progress and
costs. They're ignored on the command line.

## Testing

```sh
cd gpu && python3 -m unittest test_vast.py
```

The tests use a fake vast.ai that runs "instances" as local processes, so they check renting, the job
wrapper, downloading, the voice hand-off and destroying, but not the real vast.ai API.
