"""Voices a whole backlog of Bix's lines in one go, on the GPU it runs on.

    python batch.py --in /in --out /out [--workers auto] [--batch 16] [--stop-at EPOCH]

/in/lines.jsonl has one {"key": ..., "text": ...} per line. Each line's audio is written to
/out/<key>.<ext> as soon as it's made, and /out/voice.json says which voice and format they're in, so
whatever is done when the run stops is still usable. gpu/vast.py uploads the backlog, runs this, pulls
the audio back while it works and files it in the game server's asset store.

The model is small (0.5B) and its reference code makes one line at a time: for every 10 or so ms of
speech it runs the language model once and the diffusion head ten times, each a tiny GPU call, so the
GPU mostly waits on Python. So each copy of the model here voices --batch lines in the same calls (the
sentences side by side in one batch, padded on the left), which costs about the same per step as one
line, and several copies run in separate processes (--workers, by default as many as fit in GPU memory).
Lines of similar length go together, longest first, so no batch waits long on one long line.

It only needs the image's server.py (Voice, the backends and encode), so a running image doesn't have to
be rebuilt to use it: gpu/vast.py sends this file along.
"""

from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import os
import queue
import subprocess
import sys
import time
import traceback

# server.py is next to this file in the repo; in the image (where this runs from gpu/vast.py) it's in /app.
sys.path.insert(0, os.environ.get("TTS_APP") or (os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals()
                                                 else "/app"))
os.environ.setdefault("TQDM_DISABLE", "1")  # the reference sampler draws a progress bar per line
import server  # noqa: E402

# GPU memory one copy of the model takes while it works on a batch, with room to spare, in GB. Two copies
# voicing 16 lines each used 7.1 GB of an RTX 4060 Ti's 16 and kept it only 12-22% busy, so a 16 GB card
# takes four.
PER_WORKER_GB = 3.5
MAX_WORKERS = 6
# Lines one copy voices together.
BATCH = 24
# How long to wait on the last lines after the queue is empty and nothing has come back.
STUCK_S = 600
# Stop if this many lines fail before any works: something is wrong with the GPU or model, not the lines.
FAIL_FAST = 5


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:300]


def gpu_gb() -> float:
    """The GPU's memory, asked of nvidia-smi so this process never touches CUDA (the workers are forked)."""
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits"],
                             capture_output=True, text=True, timeout=20).stdout
        return float(out.split()[0]) / 1024
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return 0.0


def gpu_use() -> str:
    """The GPU's load and memory in use, for the progress line."""
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu,memory.used",
                              "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=10).stdout
        util, mem = (float(v) for v in out.split(",")[:2])
        return f"; GPU {util:.0f}% busy, {mem / 1024:.1f} GB in use"
    except (OSError, ValueError, subprocess.SubprocessError):
        return ""


def worker_count(asked: str, backend: str) -> int:
    if asked != "auto":
        return max(1, min(16, int(asked)))
    if backend == "stub":
        return 4
    gb = gpu_gb()
    return max(1, min(MAX_WORKERS, int((gb - 2) // PER_WORKER_GB))) if gb else 2


def work(n: int, spec: str, backend: str, fmt: str, out: str, jobs, results, size: int = BATCH):
    """One copy of the model: makes lines from `jobs`, `size` at a time, until it gets None."""
    try:
        voice = server.Voice(spec)
        model = server.StubBackend(voice) if backend == "stub" else server.MingBackend(voice)
    except BaseException as e:  # noqa: BLE001 (report anything, even SystemExit, then stop)
        results.put(("dead", n, f"{type(e).__name__}: {e}", 0.0))
        return
    if backend != "stub" and (unsupported := gpu_unsupported()):
        results.put(("dead", n, unsupported, 0.0))
        return
    results.put(("ready", n, "", 0.0))
    many = None
    if hasattr(model, "model") and size > 1:  # Ming, not the stub
        try:
            many = MingBatch(model)
        except Exception as e:  # noqa: BLE001
            results.put(("note", n, f"batching is off ({type(e).__name__}: {e}); voicing one line at a time", 0.0))
    finished = traced = False
    while not finished:
        job = jobs.get()
        if job is None:
            return
        chunk = [job]
        while len(chunk) < (size if many else 1):
            try:
                more = jobs.get_nowait()
            except queue.Empty:
                break
            if more is None:
                finished = True
                break
            chunk.append(more)
        started = time.monotonic()
        try:
            waves, many, said = voice_chunk(many, model, [text for _, text in chunk])
            if said:
                results.put(("note", n, said, 0.0))
        except Exception as e:  # a broken GPU stops everything
            for j in chunk:
                jobs.put(j)
            results.put(("fatal", n, f"{type(e).__name__}: {e}", time.monotonic() - started))
            return
        secs = (time.monotonic() - started) / len(chunk)
        for (key, _), wave in zip(chunk, waves):
            if isinstance(wave, Exception):
                results.put(("failed", n, f"{key}: {type(wave).__name__}: {wave}", secs))
                if not traced:  # the first one in full, so the log shows where it came from
                    traced = True
                    results.put(("trace", n, "".join(traceback.format_exception(wave)), 0.0))
                continue
            try:
                audio = server.encode(wave, model.sample_rate, fmt)
                path = os.path.join(out, f"{key}.{fmt}")
                with open(path + ".part", "wb") as f:
                    f.write(audio)
                os.replace(path + ".part", path)
                results.put(("made", n, key, secs))
            except Exception as e:  # noqa: BLE001
                results.put(("failed", n, f"{key}: {type(e).__name__}: {e}", secs))
    if finished:
        jobs.put(None)  # leave the stop sign for the next copy


def voice_chunk(many, model, texts: list[str]):
    """Voices `texts` with the batched sampler `many` if there is one, else one line at a time. Lines the
    batched sampler couldn't make are tried again one at a time; if that works where batching didn't, the
    batched sampler is the problem, and it's turned off. Returns the waves (or each line's exception), the
    sampler to use from now on, and a note for the log if it was turned off. A CUDA error is raised."""
    def one(text):
        try:
            return model.synthesize(text)
        except Exception as e:  # noqa: BLE001
            if "CUDA error" in str(e):
                raise
            return e

    if not many:
        return [one(t) for t in texts], None, None
    try:
        waves = many.synthesize(texts)
    except Exception as e:  # noqa: BLE001
        if "CUDA error" in str(e):
            raise
        # The batched sampler is ours, not Ming's; if it trips, the reference one still works.
        return [one(t) for t in texts], None, (f"batching failed ({type(e).__name__}: {first_line(str(e))}); "
                                               "voicing one line at a time from now on")
    bad = [i for i, w in enumerate(waves) if isinstance(w, Exception)]
    if not bad:
        return waves, many, None
    first = waves[bad[0]]
    for i in bad:
        waves[i] = one(texts[i])
    if all(isinstance(waves[i], Exception) for i in bad):
        return waves, many, None  # the lines themselves fail, batched or not
    return waves, None, (f"batching failed on {len(bad)} line(s) that work one at a time ({type(first).__name__}: "
                         f"{first_line(str(first))}); voicing one line at a time from now on")


class MingBatch:
    """Ming's sampling loop (BailingMMNativeForConditionalGeneration.sample) for several text fragments at
    once. Everything around it stays Ming's own: MingTTS.synthesize still splits and normalizes the text,
    builds the prompts and joins the pieces. It runs twice per batch: first with the model's `generate`
    noting what it's asked for, then, once those are all sampled side by side, handing back the results.
    Each prompt is padded on the left so every row's newest step is at the end; rows that finish early
    ride along until the batch is done, and their extra steps are dropped."""

    def __init__(self, backend):
        self.torch, self.voice, self.ming = backend.torch, backend.voice, backend.model
        self.max_cache = getattr(sys.modules.get(type(self.ming.model).__module__), "MAX_CACHE_LEN", 512)
        # A described voice speaks later fragments of a line like its first one, so a long line's fragments
        # can't be made side by side; those lines are made one at a time.
        self.chained = self.voice.kind in ("freeform", "design")

    def synthesize(self, texts: list[str]) -> list:
        """A waveform (or the exception it raised) for each text, as MingBackend.synthesize returns."""
        torch, m = self.torch, self.ming.model
        asked: list[list[dict]] = []

        def note(**kw):
            asked[-1].append(kw)
            # Shaped as the model's generate returns it, (1, samples): Ming trims each piece's trailing
            # silence, which needs that leading dimension.
            return torch.zeros(1, self.ming.sample_rate // 10)

        out: list = [None] * len(texts)
        m.generate = note
        try:
            for i, text in enumerate(texts):
                asked.append([])
                try:
                    self.ming.synthesize(**self.voice.kwargs(text))
                except Exception as e:  # noqa: BLE001
                    out[i] = e
        finally:
            del m.generate
        rows = [(i, kw) for i, kws in enumerate(asked) if out[i] is None and not (self.chained and len(kws) > 1)
                for kw in kws]
        torch.manual_seed(server.SEED)
        if torch.cuda.is_available():
            torch.cuda.manual_seed_all(server.SEED)
        dev = m.device
        autocast = torch.autocast(device_type=dev.type, dtype=torch.bfloat16, enabled=dev.type == "cuda")
        made: dict[int, list] = {}
        with torch.inference_mode():
            for latents, (i, _) in zip(self.sample([kw for _, kw in rows]), rows):
                with autocast:
                    speech = m.audio.decode(latents, past_key_values=None, use_cache=False, last_chunk=True)[0]
                made.setdefault(i, []).append(speech.cpu().float()[0])
        for i, text in enumerate(texts):
            if out[i] is not None:
                continue
            try:
                if i in made:
                    pieces = iter(made[i])
                    m.generate = lambda **kw: next(pieces)
                    try:
                        wave = self.ming.synthesize(**self.voice.kwargs(text))
                    finally:
                        del m.generate
                else:
                    with torch.inference_mode():
                        wave = self.ming.synthesize(**self.voice.kwargs(text))
                out[i] = wave.squeeze(0).float().cpu().numpy()
            except Exception as e:  # noqa: BLE001
                if "CUDA error" in str(e):
                    raise
                out[i] = e
        return out

    def sample(self, rows: list[dict]) -> list:
        """The latent patches for each call to `generate` (its keyword arguments), from one batched run of
        Ming's sampling loop."""
        torch, m = self.torch, self.ming.model
        if not rows:
            return []
        dev = m.device
        # Everything under bf16 autocast, as MingTTS.generate runs it (the model's weights are bf16).
        autocast = torch.autocast(device_type=dev.type, dtype=torch.bfloat16, enabled=dev.type == "cuda")
        embeds = []
        for kw in rows:
            latent = kw.get("prompt_latent")
            if kw.get("prompt_text") is None:
                latent = None
            elif latent is None and kw.get("prompt_waveform") is not None:
                latent = m.encode_prompt_latent(kw["prompt_waveform"])
            kw["prompt_latent"] = latent
            with autocast:
                _, emb = m.prepare_input_embed(
                    prompt=kw["prompt"], text=kw["text"], spk_emb=kw.get("spk_emb"),
                    instruction=kw.get("instruction"), prompt_latent=latent, prompt_text=kw.get("prompt_text"),
                    use_zero_spk_emb=kw.get("use_zero_spk_emb", False))
            embeds.append(emb[0])
        n, width = len(rows), max(e.shape[0] for e in embeds)
        inputs = torch.zeros(n, width, embeds[0].shape[-1], device=dev, dtype=embeds[0].dtype)
        mask = torch.zeros(n, width, device=dev, dtype=torch.long)
        for r, e in enumerate(embeds):
            inputs[r, width - e.shape[0]:] = e
            mask[r, width - e.shape[0]:] = 1
        position_ids = (mask.cumsum(-1) - 1).masked_fill_(mask == 0, 1)
        kw0 = rows[0]
        # As in the reference: no more steps than the 512-position cache leaves after the prompt.
        budgets = [min(kw.get("max_decode_steps", 1000), (self.max_cache - e.shape[0]) // m.patch_size)
                   for kw, e in zip(rows, embeds)]
        llm = m.model.model  # the Qwen2 body: its last hidden state, without the unused vocabulary head
        past, history = None, None
        got: list[list] = [[] for _ in rows]
        done = [False] * n
        for step in range(max(budgets)):
            with autocast:
                res = llm(attention_mask=mask, position_ids=position_ids, inputs_embeds=inputs,
                          use_cache=True, past_key_values=past, return_dict=True)
                past = res.past_key_values
                z = res.last_hidden_state[:, -1:, :]
                if step == 0:
                    history = torch.zeros(n, m.history_patch_size, m.latent_dim, device=z.device, dtype=z.dtype)
                    for r, kw in enumerate(rows):
                        pl = kw["prompt_latent"]
                        if pl is not None:
                            pl = pl.to(z.dtype)
                            start = m.history_patch_size - pl.size(1)
                            if start < 0:
                                history[r] = pl[0, -start:, :]
                            else:
                                history[r, start:, :] = pl[0]
                latent = m.flowloss.sample(z, history, kw0.get("cfg", 2.0), m.patch_size,
                                           sigma=kw0.get("sigma", 0.25), temperature=kw0.get("temperature", 0))
                stop = (m.stop_head(z)[:, 0].softmax(dim=-1)[:, 1] > 0.5).tolist()
            for r in range(n):
                if done[r]:
                    continue
                got[r].append(latent[r:r + 1])
                if (stop[r] and step > 3) or step + 1 >= budgets[r]:
                    done[r] = True
            if all(done):
                break
            with autocast:
                inputs = m.linear_proj_audio(latent)
            position_ids = position_ids[:, -1:] + 1
            mask = torch.cat([mask, torch.ones(n, 1, device=dev, dtype=mask.dtype)], dim=1)
            history[:, :-m.patch_size, :] = history[:, m.patch_size:, :].clone()
            history[:, -m.patch_size:, :] = latent
        return [torch.cat(g, dim=1) for g in got]


def gpu_unsupported() -> str | None:
    """Why this PyTorch build can't run on this GPU, if it can't (say, a newer card than the build knows,
    which fails every line with "no kernel image is available"). Checked before any work is taken."""
    try:
        import torch
        if not torch.cuda.is_available():
            return "no CUDA GPU is visible"
        major, minor = torch.cuda.get_device_capability()
        arches = torch.cuda.get_arch_list()
    except Exception as e:  # noqa: BLE001
        return f"couldn't check the GPU: {type(e).__name__}: {e}"
    if arch_supported(major, minor, arches):
        return None
    # The list can be incomplete, so a card it doesn't cover still gets a real try.
    try:
        (torch.ones(64, device="cuda") * 2).sum().item()
        return None
    except Exception as e:  # noqa: BLE001
        return (f"this PyTorch build ({torch.__version__}) can't run on {torch.cuda.get_device_name()} "
                f"(sm_{major}{minor}; built for {', '.join(arches)}): {first_line(str(e))}")


def arch_supported(major: int, minor: int, arches: list[str]) -> bool:
    """Whether a PyTorch built for `arches` (as torch.cuda.get_arch_list() names them) runs on a GPU of
    compute capability major.minor: kernels for the same major version and an equal or lower minor (an sm_86
    build runs on sm_89), or PTX for an equal or older one, which the driver compiles up."""
    for a in arches:
        kind, _, num = a.partition("_")
        if not num.isdigit():
            continue
        built = divmod(int(num), 10)
        if kind == "sm" and built[0] == major and built[1] <= minor:
            return True
        if kind == "compute" and built <= (major, minor):
            return True
    return False


def load_lines(folder: str) -> list[tuple[str, str]]:
    lines, seen = [], set()
    with open(os.path.join(folder, "lines.jsonl"), encoding="utf-8") as f:
        for row in f:
            if not row.strip():
                continue
            item = json.loads(row)
            key, text = str(item["key"]), str(item["text"]).strip()
            if key in seen or not text or len(text) > server.MAX_TEXT or not key.isalnum():
                continue
            seen.add(key)
            lines.append((key, text))
    lines.sort(key=lambda kt: -len(kt[1]))
    return lines


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--in", dest="inp", default="/in")
    ap.add_argument("--out", default="/out")
    ap.add_argument("--workers", default=os.environ.get("TTS_WORKERS", "auto"))
    ap.add_argument("--batch", type=int, default=int(os.environ.get("TTS_BATCH") or BATCH),
                    help=f"lines each copy voices together (default {BATCH}; 1 uses Ming's own one-line sampler)")
    ap.add_argument("--stop-at", type=float, default=0, help="stop by this time (epoch seconds), keeping what's made")
    args = ap.parse_args(argv)

    spec = os.environ.get("BIX_VOICE", "") or "ip:US_James"
    voice = server.Voice(spec)
    backend = os.environ.get("TTS_BACKEND", "ming").lower()
    fmt = os.environ.get("TTS_FORMAT", "ogg").lower()
    if backend == "stub":
        fmt = "wav"  # the stub has no soundfile to encode with
    model = os.environ.get("TTS_MODEL", "inclusionAI/Ming-3.0-tts-0.5b") if backend != "stub" else "stub"
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "voice.json"), "w") as f:
        json.dump({"voice": voice.key, "format": fmt, "content_type": server.CONTENT_TYPES[fmt], "model": model}, f)

    lines = load_lines(args.inp)
    todo = [kt for kt in lines if not os.path.exists(os.path.join(args.out, f"{kt[0]}.{fmt}"))]
    total = len(todo)
    count = min(worker_count(args.workers, backend), max(1, total))
    print(f"[batch] {total} lines in {voice.key}, up to {count} copies of the model at once, "
          f"{args.batch} lines at a time each", flush=True)
    if not total:
        return 0

    # Forked, so this process must not start CUDA itself; each worker loads its own copy of the model.
    ctx = mp.get_context("fork")
    jobs, results = ctx.Queue(), ctx.Queue()
    for kt in todo:
        jobs.put(kt)
    procs = {}

    def start(n):
        p = ctx.Process(target=work, args=(n, spec, backend, fmt, args.out, jobs, results, args.batch), daemon=True)
        p.start()
        procs[n] = p

    # The first copy loads alone (the first start downloads the model), then the rest join it.
    start(0)
    made = failed = ready = dead = 0
    busy_s = 0.0
    began = time.monotonic()
    first_at = None
    last_note = 0.0
    last_result = time.monotonic()
    stopped = None
    errors: dict[str, int] = {}
    while made + failed < total:
        if args.stop_at and time.time() > args.stop_at:
            stopped = "the time limit is near"
            break
        try:
            what, n, info, secs = results.get(timeout=5)
        except queue.Empty:
            if all(not p.is_alive() for p in procs.values()):
                stopped = "every worker stopped"
                break
            # A copy that crashed mid-line takes that line with it; don't wait for it forever.
            if ready and jobs.empty() and time.monotonic() - last_result > STUCK_S:
                stopped = "the last lines never came back"
                break
            continue
        last_result = time.monotonic()
        if what == "ready":
            ready += 1
            if ready == 1:
                first_at = time.monotonic()
                print(f"[batch] model loaded in {first_at - began:.0f}s", flush=True)
                for m in range(1, count):
                    start(m)
            else:
                print(f"[batch] {ready} copies working", flush=True)
        elif what == "dead":
            dead += 1
            print(f"[batch] copy {n} couldn't load: {info}", flush=True)
            if ready == 0 and n == 0:
                return 1
        elif what == "trace":
            for line in info.rstrip().splitlines():
                print(f"[batch] copy {n}: {line}", flush=True)
        elif what == "note":
            if info not in errors:
                errors[info] = 0
                print(f"[batch] copy {n}: {info}", flush=True)
        elif what == "fatal":
            print(f"[batch] stopping: the GPU failed in a way that every line would ({first_line(info)})", flush=True)
            stopped = "the GPU can't run the model"
            break
        elif what == "made":
            made += 1
            busy_s += secs
        elif what == "failed":
            failed += 1
            key, _, said = info.partition(": ")
            said = first_line(said)
            errors[said] = errors.get(said, 0) + 1
            if errors[said] == 1:
                print(f"[batch] failed {key}: {said}", flush=True)
            if made == 0 and failed >= FAIL_FAST:
                stopped = f"the first {failed} lines all failed"
                break
        now = time.monotonic()
        if now - last_note > 60 or made + failed == total:
            last_note = now
            mins = (now - (first_at or began)) / 60
            rate = made / mins if mins > 0 else 0
            left = total - made - failed
            eta = f", about {left / rate:.0f} min to go" if rate and left else ""
            print(f"[batch] voiced {made} of {total} ({rate:.0f} lines/min on {ready} copies{eta}){gpu_use()}",
                  flush=True)
    for _ in procs:
        jobs.put(None)
    for p in procs.values():
        p.join(timeout=10)
        if p.is_alive():
            p.kill()
    secs = time.monotonic() - (first_at or began)
    with open(os.path.join(args.out, "stats.json"), "w") as f:
        json.dump({"made": made, "failed": failed, "total": total, "workers": ready, "seconds": round(secs, 1),
                   "load_seconds": round((first_at or began) - began, 1), "busy_seconds": round(busy_s, 1)}, f)
    for said, times in errors.items():
        if times > 1:
            print(f"[batch] {times} lines failed with: {said}", flush=True)
    print(f"[batch] done: {made} made, {failed} failed of {total} in {secs / 60:.1f} min"
          + (f"; stopped because {stopped}" if stopped else ""), flush=True)
    return 0 if made == total else 1


if __name__ == "__main__":
    sys.exit(main())
