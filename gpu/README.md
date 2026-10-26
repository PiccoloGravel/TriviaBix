# Rented GPUs (vast.ai)

The Unraid box doesn't need a GPU. When there's something to generate, `vast.py` rents one on
[vast.ai](https://vast.ai), does the work, saves the results at home, and destroys the instance. A 24 GB
card such as an RTX 4090 is usually $0.25 to $0.45 an hour, so making all of Bix's lines once costs cents.
Everything made is kept in the asset store ([docs/assets.md](../docs/assets.md)) and never made twice.

It's one Python 3 file with no dependencies, so it runs anywhere: a laptop, or a throwaway container on
Unraid (below).

## Setting up, once

1. Make a vast.ai account and **add a small amount of credit, such as $5 to $10**. vast.ai is prepaid, so
   that credit is the hard limit on what can ever be spent. Leave auto-reload off.
2. Make an API key at <https://cloud.vast.ai/manage-keys/>. Keep it out of the repo and out of chat; set it
   as `VAST_API_KEY` where you run the script (on Unraid, in the User Scripts script or the container's
   variables).
3. vast.ai has to be able to pull the voice image. After the first GitHub Actions build, make the package
   public: GitHub → your profile → Packages → `triviabix-tts` → Package settings → Change visibility →
   Public. (Or keep it private and pass `--login "-u PiccoloGravel -p <a read:packages token> ghcr.io"`.)

## Bix's voice

```sh
export VAST_API_KEY=...  ADMIN_PASSWORD=<the game's admin password>
python3 gpu/vast.py voice --server http://192.168.1.20:8080 --dry-run   # shows the GPU and the price
python3 gpu/vast.py voice --server http://192.168.1.20:8080             # does it
```

It logs in to the game server's admin API, checks how many lines are waiting (and rents nothing if none
are), rents the cheapest suitable GPU, starts the `triviabix-tts` image there with a one-time password,
and points the game server's voice at it. The server sends it every missing line; the audio comes back
into `appdata/triviabix/assets/voice`. When the backlog is empty it points the server back at `TTS_URL`
(or none) and destroys the instance. Cached lines keep playing with no GPU at all; only new lines with
player names fall back to the browser voice.

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

Built-in voices only for now (`--voice ip:UK_George` and so on). A cloned `bix.wav` voice would need the
clip on the rented GPU, which isn't wired up yet.

## Any other job (images, video)

```sh
python3 gpu/vast.py run --image ghcr.io/piccologravel/triviabix-art:latest \
  --cmd "python generate.py --out /out" --out /out --to ./art-output --min-vram 24 --max-minutes 45
```

It runs the command in the image, tars `/out` when it exits, downloads it into `--to`, and destroys the
instance. The exit code is the job's. The image needs `python3` and no `ENTRYPOINT` (the command is
started with `python3 -c`). From Python: `from vast import run_job`. Add the results to the asset store
with `python3 gpu/assets.py import ./art-output --kind image --style <style>`.

## Limits on spending

- **Price:** `--max-price` is the most per hour, all in (default $0.50). Only verified, reliable machines
  with a single Ampere-or-newer GPU and fast internet are considered, cheapest first.
- **Time:** `--max-hours` (voice, default 3) and `--max-minutes` (jobs, default 60). Before renting, it
  prints the most the run can cost and asks; pass `--yes` to skip the question in scripts.
- **Always destroyed:** when the work ends, fails, hits the time limit, or you press Ctrl-C. As a backstop
  the instance also destroys itself at the time limit, so it stops even if the computer running the
  script goes to sleep or loses its connection. If it can't, it exits, which stops the GPU charge; a
  stopped instance's disk costs pennies a day until `cleanup`.
- **One at a time:** it won't rent a second TriviaBix instance while one is running (`--another` to
  override).
- `python3 gpu/vast.py list` shows what's running and its price; `python3 gpu/vast.py cleanup` destroys
  every TriviaBix instance. Instances are labeled `triviabix-...` and nothing else on the account is
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

## Testing

```sh
cd gpu && python3 -m unittest test_vast.py
```

The tests use a fake vast.ai that runs "instances" as local processes, so they check renting, the job
wrapper, downloading, the voice hand-off and destroying, but not the real vast.ai API.
