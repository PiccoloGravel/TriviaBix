# Bix's voice (Ming-3.0-tts)

An optional service that gives Bix a real voice with
[inclusionAI/Ming-3.0-tts-0.5b](https://huggingface.co/inclusionAI/Ming-3.0-tts-0.5b) (MIT licensed,
code and weights). Without it, the TV uses the browser's built-in text-to-speech, as before.

## How it fits together

- The game server sends the TV each host line in pieces, each with an `/api/voice/<hash>` link, plus one
  for the question being read out.
- The game server asks this service for the audio the first time a line is needed and keeps it in
  the asset store (`/data/assets/voice` in Docker, see [docs/assets.md](../docs/assets.md)), one folder per voice. Every later game plays it from disk.
- At startup it queues every line it can know in advance: the host script's lines without names or
  scores in them, every question, and every question's own lead-in and reveal lines. They're made in the
  background, one at a time, when a voice service is set. Without one, a voice run (admin Pipeline, or
  `gpu/vast.py voice`) makes them all at once on a rented GPU with `batch.py`, several copies of the model
  working side by side. After the first run only new or edited lines are made.
- When a game starts, that game's questions and lines jump to the front of the queue. Editing a line in
  `/admin/questions` queues the new text right away.
- Lines with live details (player names, scores, the round's category) are made when the TV asks for
  them. The TV starts loading every piece of a line at once, so later pieces are made while the first one
  plays. Anything that isn't ready within 3.5 seconds, or fails, is read by the browser voice instead, so
  the game never waits on the GPU.
- On the TV, **V** cycles from "Bix (Ming TTS)" through the browser's voices, and **M** mutes.

## Running it

It needs an NVIDIA GPU, the NVIDIA driver and the NVIDIA container toolkit (on Unraid: the Nvidia Driver
plugin). From the repo root:

```sh
TTS_URL=http://tts:8000 docker compose --profile voice up --build
```

The first start downloads the model (about 3 GB) into the `tts-models` volume, then
`http://<server>:8000/healthz` shows `"ready": true`. Until then, and whenever the service is down, lines
already made keep playing and everything else uses the browser voice.

On Unraid with bridge networking, run this container separately (publish port 8000, pass the GPU with
`--runtime=nvidia` and `NVIDIA_VISIBLE_DEVICES`) and set the game server's `TTS_URL` to
`http://<unraid-ip>:8000`.

No NVIDIA GPU? [gpu/README.md](../gpu/README.md) runs this same image on a rented vast.ai GPU, pointed at
your game server until the lines are made.

### Settings

| Variable | Default | |
|---|---|---|
| `BIX_VOICE` | `ip:US_James` | `ip:<name>` for a built-in voice (`US_James`, `US_Emma`, `UK_George`, `UK_Charlotte`); `clone:/voices/bix.wav` to clone a reference clip; `freeform:<instruction>` or `design:<description>` to describe one (these can drift from line to line, so prefer making a clip, below) |
| `TTS_FORMAT` | `ogg` | `ogg` (small), `flac` or `wav` |
| `TTS_DEVICE` | `cuda:0` | `cpu` works in principle but is untested and slow: expect only pre-made lines to come out in Bix's voice |
| `TTS_MODEL` | `inclusionAI/Ming-3.0-tts-0.5b` | A Hub id or a local folder |
| `TTS_TOKEN` | | When set, `/v1/speech` needs `Authorization: Bearer <token>`. `gpu/vast.py` sets one on rented GPUs |
| `TTS_BACKEND` | `ming` | `stub` makes beeps instead, for testing without a GPU |

Changing the voice makes a fresh cache folder, so everything is regenerated in the new voice.

## A custom voice for Bix

The built-in voices are clean narrators. For a cheekier host, design one and clone it so every line sounds
like the same person. The easy way is the admin panel: **Pipeline → Design Bix's voice**. Describe him,
keep or edit the list of lines he might say (each candidate says a different one, picked at random), check the price, and it makes a few candidates on a rented GPU. Listen to them there and press **Use this
voice**; the next voice run (step 3) makes his lines again in it. Candidates are kept in the asset store,
so going back to one costs nothing.

On a machine with its own GPU:

```sh
docker compose --profile voice run --rm tts python make_voice.py --count 4
```

That writes `voices/bix-1.wav` to `bix-4.wav` (with transcripts) from a "washed-up game show host"
description. Listen to them, rename your favorite to `voices/bix.wav` and its transcript to
`voices/bix.txt`, then restart with `BIX_VOICE=clone:/voices/bix.wav`. Change the description with
`--description "..."`. You can also use your own 5 to 15 second recording with an exact transcript, but
only of someone who has agreed to it.

With a cloned voice, script lines can include Ming's sound effects, such as
`Oh no. [Laughter] Not even close.` The tags are `[Laughter]`, `[Sigh]`, `[Breathing]`, `[Cough]`, `[Uhm]`,
`[Surprise-ah]`, `[Surprise-oh]`, `[Dissatisfaction-hnn]`, `[Confirmation-en]` and `[Question-ei|ah|en|oh|huh]`.
The TV hides them in the speech bubble, and they're dropped for built-in voices and the browser voice.

## Testing without a GPU

```sh
cd tts && python3 -m unittest test_server.py      # the service, with the stub backend
TTS_BACKEND=stub python3 tts/server.py            # a beeping Bix on :8000 for trying the game
```

`cargo test` covers the game server's side with a fake service.
