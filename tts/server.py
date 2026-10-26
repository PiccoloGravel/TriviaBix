"""Bix's voice: a small HTTP service that turns host lines into speech with Ming-3.0-tts.

    POST /v1/speech  {"text": "...", "priority": "live" | "warm"}  -> audio (Ogg Vorbis by default)
    GET  /healthz    -> {"ok", "ready", "backend", "voice", "format"}

The game server calls this and caches what comes back, so each line is only made once. One line
is synthesized at a time (the GPU is the bottleneck anyway); "live" requests for lines a TV is
waiting on jump ahead of "warm" ones from the background pre-generation queue.

Settings (environment):
    TTS_BACKEND   ming (default) or stub, a beep generator for tests and machines without a GPU
    TTS_MODEL     Hub repo id or local folder, default inclusionAI/Ming-3.0-tts-0.5b
    TTS_DEVICE    cuda:0 (default) or cpu
    TTS_FORMAT    ogg (default), flac or wav
    BIX_VOICE     ip:US_James (default), clone:/voices/bix.wav, freeform:<instruction> or design:<style>
    PORT          8000
"""

from __future__ import annotations

import hashlib
import io
import json
import logging
import math
import os
import re
import sys
import threading
import time
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

log = logging.getLogger("bix-tts")

MAX_TEXT = 1000
SEED = 1895
CONTENT_TYPES = {"ogg": "audio/ogg", "flac": "audio/flac", "wav": "audio/wav"}

# Ming's inline sound effects. Only the cloning task (TTS) knows them; other tasks get them stripped.
TAG_RE = re.compile(
    r"\[(Breathing|Cough|Sigh|Laughter|Uhm|Surprise-ah|Surprise-oh|Dissatisfaction-hnn|"
    r"Confirmation-en|Question-ei|Question-ah|Question-en|Question-oh|Question-huh)\]"
)


def strip_tags(text: str) -> str:
    return re.sub(r"\s{2,}", " ", TAG_RE.sub("", text)).strip()


class Voice:
    """Parses BIX_VOICE into the arguments MingTTS.synthesize wants for every line."""

    def __init__(self, spec: str):
        self.spec = spec.strip() or "ip:US_James"
        kind, _, value = self.spec.partition(":")
        self.kind, self.value = kind.strip().lower(), value.strip()
        if self.kind not in ("ip", "clone", "freeform", "design") or not self.value:
            raise SystemExit(f"BIX_VOICE={spec!r}: use ip:<name>, clone:<wav>, freeform:<text> or design:<text>")
        self.prompt_text = None
        if self.kind == "clone":
            transcript = os.path.splitext(self.value)[0] + ".txt"
            if not os.path.exists(self.value) or not os.path.exists(transcript):
                raise SystemExit(f"BIX_VOICE clone needs {self.value} and its transcript {transcript}")
            with open(transcript, encoding="utf-8") as f:
                self.prompt_text = f.read().strip()
        # Names the voice for the game server's cache, so a new voice (or a new recording) means new audio.
        self.key = self.spec
        if self.kind == "clone":
            with open(self.value, "rb") as f:
                h = hashlib.sha256(f.read())
            h.update(self.prompt_text.encode())
            self.key = f"clone:{os.path.basename(self.value)}:{h.hexdigest()[:12]}"

    def kwargs(self, text: str) -> dict:
        if self.kind == "clone":
            return dict(task="TTS", text=text, language_id="English", prompt_wav_path=self.value,
                        prompt_text=self.prompt_text, use_spk_emb=True)
        text = strip_tags(text)
        if self.kind == "ip":
            return dict(task="IP", text=text, controls={"ip": self.value}, use_zero_spk_emb=True)
        if self.kind == "freeform":
            return dict(task="FREEFORM", text=text, instruction=self.value, use_zero_spk_emb=True)
        return dict(task="VOICE_DESIGN", text=text, controls={"style": self.value}, use_zero_spk_emb=True)


class StubBackend:
    """A rising beep about as long as the line would take to say. Lets everything around the model
    be built and tested on a machine without a GPU."""

    name = "stub"
    sample_rate = 16000

    def __init__(self, voice: Voice):
        self.voice = voice
        self.delay = float(os.environ.get("TTS_STUB_DELAY", "0"))

    def synthesize(self, text: str) -> list[float]:
        time.sleep(self.delay)
        secs = min(8.0, 0.3 + len(strip_tags(text)) / 15)
        n = int(secs * self.sample_rate)
        return [0.2 * math.sin(2 * math.pi * (300 + 200 * i / n) * i / self.sample_rate) for i in range(n)]


class MingBackend:
    name = "ming"

    def __init__(self, voice: Voice):
        sys.path.insert(0, os.environ.get("MING_DIR", "/opt/ming"))
        import torch
        from ming_tts import MingTTS

        try:
            import pynini  # noqa: F401  (numbers, dates and units are read out properly with it)
            normalize = True
        except ImportError:
            log.warning("pynini isn't installed, so text normalization is off")
            normalize = False
        self.torch = torch
        self.voice = voice
        model = os.environ.get("TTS_MODEL", "inclusionAI/Ming-3.0-tts-0.5b")
        device = os.environ.get("TTS_DEVICE", "cuda:0")
        log.info("loading %s on %s", model, device)
        self.model = MingTTS(model, device=device, text_normalization=normalize)
        self.sample_rate = self.model.sample_rate

    def synthesize(self, text: str):
        # The same line always comes out the same.
        self.torch.manual_seed(SEED)
        if self.torch.cuda.is_available():
            self.torch.cuda.manual_seed_all(SEED)
        with self.torch.inference_mode():
            waveform = self.model.synthesize(**self.voice.kwargs(text))
        return waveform.squeeze(0).float().cpu().numpy()


def encode(samples, sample_rate: int, fmt: str) -> bytes:
    buf = io.BytesIO()
    if fmt == "wav":
        frames = bytearray()
        for s in samples:
            frames += int(max(-1.0, min(1.0, float(s))) * 32767).to_bytes(2, "little", signed=True)
        with wave.open(buf, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(sample_rate)
            w.writeframes(bytes(frames))
    else:
        import soundfile

        subtype = "VORBIS" if fmt == "ogg" else "PCM_16"
        soundfile.write(buf, samples, sample_rate, format=fmt.upper(), subtype=subtype)
    return buf.getvalue()


class Gate:
    """One synthesis at a time, with live requests ahead of warm-up ones."""

    def __init__(self):
        self.cond = threading.Condition()
        self.busy = False
        self.live_waiting = 0

    def __call__(self, live: bool):
        gate = self

        class Hold:
            def __enter__(self):
                with gate.cond:
                    if live:
                        gate.live_waiting += 1
                    while gate.busy or (not live and gate.live_waiting):
                        gate.cond.wait()
                    if live:
                        gate.live_waiting -= 1
                    gate.busy = True

            def __exit__(self, *exc):
                with gate.cond:
                    gate.busy = False
                    gate.cond.notify_all()

        return Hold()


class Service:
    def __init__(self):
        self.fmt = os.environ.get("TTS_FORMAT", "ogg").lower()
        if self.fmt not in CONTENT_TYPES:
            raise SystemExit(f"TTS_FORMAT={self.fmt!r}: use ogg, flac or wav")
        self.voice = Voice(os.environ.get("BIX_VOICE", ""))
        self.backend_name = os.environ.get("TTS_BACKEND", "ming").lower()
        if self.backend_name == "stub" and self.fmt != "wav":
            self.fmt = "wav"  # the stub has no soundfile to encode with
        self.backend = None
        self.error = None
        self.gate = Gate()

    def load(self):
        try:
            cls = StubBackend if self.backend_name == "stub" else MingBackend
            self.backend = cls(self.voice)
            log.info("ready: %s backend, voice %s, %s audio", self.backend_name, self.voice.spec, self.fmt)
        except Exception as e:  # keep serving /healthz so the problem shows up there
            log.exception("couldn't load the TTS backend")
            self.error = f"{type(e).__name__}: {e}"

    def health(self) -> dict:
        return {"ok": self.error is None, "ready": self.backend is not None, "backend": self.backend_name,
                "voice": self.voice.key, "format": self.fmt, "error": self.error}

    def speak(self, text: str, live: bool) -> bytes:
        with self.gate(live):
            started = time.monotonic()
            samples = self.backend.synthesize(text)
            audio = encode(samples, self.backend.sample_rate, self.fmt)
        log.info("%s %.2fs: %s", "live" if live else "warm", time.monotonic() - started, text[:80])
        return audio


def make_handler(service: Service):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            log.debug(fmt, *args)

        def reply(self, status: int, body: bytes, content_type: str, headers: dict | None = None):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            for k, v in (headers or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, obj: dict):
            self.reply(status, json.dumps(obj).encode(), "application/json")

        def do_GET(self):
            if self.path == "/healthz":
                return self.json(200, service.health())
            self.json(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/v1/speech":
                return self.json(404, {"error": "not found"})
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
                text = str(body.get("text", "")).strip()
            except (ValueError, AttributeError):
                return self.json(400, {"error": "send JSON like {\"text\": \"...\"}"})
            if not text or len(text) > MAX_TEXT:
                return self.json(400, {"error": f"text must be 1 to {MAX_TEXT} characters"})
            if service.backend is None:
                return self.json(503, {"error": service.error or "still loading the model"})
            try:
                audio = service.speak(text, body.get("priority") != "warm")
            except Exception as e:
                log.exception("synthesis failed for %r", text[:80])
                return self.json(500, {"error": f"{type(e).__name__}: {e}"})
            self.reply(200, audio, CONTENT_TYPES[service.fmt], {"X-Voice": service.voice.key})

    return Handler


def main():
    logging.basicConfig(level=os.environ.get("LOG_LEVEL", "INFO"), format="%(asctime)s %(levelname)s %(message)s")
    service = Service()
    port = int(os.environ.get("PORT", "8000"))
    server = ThreadingHTTPServer(("0.0.0.0", port), make_handler(service))
    # Answer /healthz while the model loads; that takes a while the first time (a ~3 GB download).
    threading.Thread(target=service.load, daemon=True).start()
    log.info("listening on :%d", port)
    server.serve_forever()


if __name__ == "__main__":
    main()
