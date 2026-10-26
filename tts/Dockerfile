# Bix's voice: Ming-3.0-tts-0.5b behind a small HTTP API. Needs an NVIDIA GPU.
FROM pytorch/pytorch:2.6.0-cuda12.4-cudnn9-runtime

RUN apt-get update && apt-get install -y --no-install-recommends git libsndfile1 \
 && rm -rf /var/lib/apt/lists/*

# Ming's inference code (MIT). Set MING_REF to a tag or branch to pin it.
ARG MING_REF=main
RUN git clone --depth 1 --branch "$MING_REF" https://github.com/inclusionAI/Ming-3.0-tts.git /opt/ming

COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt \
 # Reads numbers, dates and units properly. Optional: the service runs without it.
 && (pip install --no-cache-dir pynini==2.1.6 || echo "pynini unavailable; text normalization off")

COPY server.py make_voice.py /app/
WORKDIR /app
# The model (~3 GB) downloads into /models on first start; keep it on a volume.
ENV MING_DIR=/opt/ming \
    HF_HOME=/models \
    TTS_MODEL=inclusionAI/Ming-3.0-tts-0.5b \
    BIX_VOICE=ip:US_James \
    PORT=8000 \
    PYTHONUNBUFFERED=1
VOLUME ["/models", "/voices"]
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10m \
  CMD python -c "import json,urllib.request,sys; sys.exit(0 if json.load(urllib.request.urlopen('http://127.0.0.1:8000/healthz'))['ready'] else 1)"
CMD ["python", "server.py"]
