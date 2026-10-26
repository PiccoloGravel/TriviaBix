"""Designs a voice for Bix and saves it as a reference clip the service can clone, so every line
sounds like the same cheeky host.

    docker compose --profile voice run --rm tts python make_voice.py --count 4

writes /voices/bix-1.wav ... bix-4.wav, each with a .txt transcript. Listen, keep the one you
like as /voices/bix.wav (with its .txt renamed to bix.txt), then set BIX_VOICE=clone:/voices/bix.wav.
You can also drop in your own recording (5 to 15 seconds, one speaker, with an exact transcript),
but only of someone who has agreed to it.

The admin panel's Pipeline page runs this on a rented GPU (gpu/vast.py design) with --seeds, keeps
the clips in the asset store, and lets you listen and pick one there instead.
"""

import argparse
import os
import sys

import soundfile
import torch

DESCRIPTION = (
    "A washed-up American game show host in his fifties, clinging to his last gig. His voice is warm and "
    "a little gravelly, cheeky and quick but world-weary, with dry, sarcastic timing and a tired sigh in it, "
    "and the old showman's bounce still comes back for the big moments. Always clear and easy to follow."
)
SAMPLE = (
    "Welcome back to TriviaBix. I'm Bix, your host. Still. Twenty years in show business, "
    "and this is the room they give me. Phones out, brains on. Let's play!"
)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", default="/voices", help="folder to write bix-N.wav into")
    parser.add_argument("--count", type=int, default=4, help="how many candidate voices to make")
    parser.add_argument("--seeds", help="make exactly these candidates, e.g. 1,3 (instead of 1 to --count)")
    parser.add_argument("--description", default=DESCRIPTION, help="what Bix should sound like")
    parser.add_argument("--text", default=SAMPLE, help="what he says in the reference clip")
    parser.add_argument("--mode", choices=("freeform", "design"), default="freeform")
    args = parser.parse_args()

    sys.path.insert(0, os.environ.get("MING_DIR", "/opt/ming"))
    from ming_tts import MingTTS

    model = MingTTS(os.environ.get("TTS_MODEL", "inclusionAI/Ming-3.0-tts-0.5b"),
                    device=os.environ.get("TTS_DEVICE", "cuda:0"), text_normalization=False)
    os.makedirs(args.out, exist_ok=True)
    seeds = [int(n) for n in args.seeds.split(",")] if args.seeds else range(1, args.count + 1)
    for n in seeds:
        torch.manual_seed(n)
        if args.mode == "freeform":
            wav = model.synthesize(task="FREEFORM", text=args.text, instruction=args.description,
                                   use_zero_spk_emb=True, raw_text=True)
        else:
            wav = model.synthesize(task="VOICE_DESIGN", text=args.text, controls={"style": args.description},
                                   use_zero_spk_emb=True, raw_text=True)
        path = os.path.join(args.out, f"bix-{n}.wav")
        soundfile.write(path, wav.squeeze(0).float().cpu().numpy(), model.sample_rate)
        with open(path[:-4] + ".txt", "w", encoding="utf-8") as f:
            f.write(args.text + "\n")
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
