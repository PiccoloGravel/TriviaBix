# The art generator for a rented GPU (render_on_vast.py runs it through gpu/vast.py).
# Build from the repo root:  docker build -f art/Dockerfile -t triviabix-art .
# Models are not baked in (about 45 GB); they download from Hugging Face on first use, into HF_HOME.
FROM pytorch/pytorch:2.7.1-cuda12.8-cudnn9-runtime

RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY art/requirements.txt art/requirements.txt
RUN pip install --no-cache-dir -r art/requirements.txt

COPY art art
COPY content/categories.toml content/categories.toml

ENV HF_HOME=/root/.cache/huggingface \
    HF_HUB_ENABLE_HF_TRANSFER=0 \
    PYTHONUNBUFFERED=1
CMD ["python3", "art/generate.py", "--help"]
