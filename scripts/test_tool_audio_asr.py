#!/usr/bin/env python3
"""Test /tool/audio_asr with local wav using stdlib only."""

from __future__ import annotations

import argparse
import json
import uuid
import urllib.request
from pathlib import Path


def read_key(env_path: Path, explicit: str | None) -> str:
    if explicit and explicit.strip():
        return explicit.strip()
    if not env_path.exists():
        raise RuntimeError(f"env file not found: {env_path}")
    for line in env_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("DASHSCOPE_API_KEY="):
            return line.split("=", 1)[1].strip()
    raise RuntimeError("DASHSCOPE_API_KEY not found")


def post_audio(api: str, wav_path: Path, key: str, model: str, timeout_sec: int) -> tuple[int, str]:
    boundary = "----WebKitFormBoundary" + uuid.uuid4().hex
    parts: list[bytes] = []

    def add_field(name: str, val: str) -> None:
        parts.append(f"--{boundary}\r\n".encode())
        parts.append(f"Content-Disposition: form-data; name=\"{name}\"\r\n\r\n".encode())
        parts.append(val.encode())
        parts.append(b"\r\n")

    add_field("dashscope_api_key", key)
    add_field("model", model)

    parts.append(f"--{boundary}\r\n".encode())
    parts.append(
        f"Content-Disposition: form-data; name=\"file\"; filename=\"{wav_path.name}\"\r\n".encode()
    )
    parts.append(b"Content-Type: audio/wav\r\n\r\n")
    parts.append(wav_path.read_bytes())
    parts.append(b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())

    body = b"".join(parts)
    req = urllib.request.Request(api, data=body, method="POST")
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    req.add_header("Accept", "application/json")

    with urllib.request.urlopen(req, timeout=timeout_sec) as resp:
        text = resp.read().decode("utf-8", errors="replace")
        return resp.status, text


def main() -> int:
    parser = argparse.ArgumentParser(description="Test /tool/audio_asr")
    parser.add_argument("--api", default="http://101.34.207.228:81/tool/audio_asr")
    parser.add_argument("--wav", required=True)
    parser.add_argument("--model", default="fun-asr")
    parser.add_argument("--api-key", default=None)
    parser.add_argument("--env", default="/Users/pxy/PycharmProjects/video_cut_software/.env")
    parser.add_argument("--out", default="/tmp/tool_audio_asr_test.json")
    parser.add_argument("--timeout-sec", type=int, default=1200)
    args = parser.parse_args()

    wav = Path(args.wav).expanduser().resolve()
    if not wav.exists():
        raise RuntimeError(f"wav not found: {wav}")

    key = read_key(Path(args.env).expanduser().resolve(), args.api_key)
    status, text = post_audio(args.api, wav, key, args.model, args.timeout_sec)

    out_path = Path(args.out).expanduser().resolve()
    out_path.write_text(text, encoding="utf-8")

    print(f"status: {status}")
    try:
        data = json.loads(text)
    except Exception:
        print(f"invalid json, head: {text[:200]}")
        print(f"saved: {out_path}")
        return 1

    segments = data.get("segments", []) if isinstance(data, dict) else []
    print(f"segments: {len(segments)}")
    if segments:
        print(f"first: {segments[0]}")
    print(f"saved: {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
