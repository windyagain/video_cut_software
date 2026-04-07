#!/usr/bin/env python3
"""
Use Alibaba Cloud Model Studio (DashScope) FunASR to transcribe remote audio URL,
and convert result to burn-subtitle JSON format:

{
  "language": "zh",
  "segments": [{"start": 0.0, "end": 1.23, "text": "..."}]
}

Notes:
- This script does NOT upload files to OSS.
- Input audio must be a publicly accessible HTTP/HTTPS URL.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import ssl
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


def _to_dict(obj: Any) -> dict[str, Any]:
    if isinstance(obj, dict):
        return obj
    if hasattr(obj, "to_dict"):
        data = obj.to_dict()
        if isinstance(data, dict):
            return data
    if hasattr(obj, "__dict__"):
        data = dict(obj.__dict__)
        return data
    return {}


def _extract_output(response: Any) -> dict[str, Any]:
    if response is None:
        return {}
    if isinstance(response, dict):
        out = response.get("output")
        return out if isinstance(out, dict) else {}

    output = getattr(response, "output", None)
    output_dict = _to_dict(output)
    if output_dict:
        return output_dict

    response_dict = _to_dict(response)
    out = response_dict.get("output")
    return out if isinstance(out, dict) else {}


def _response_ok(response: Any) -> bool:
    status = getattr(response, "status_code", None)
    if status is None and isinstance(response, dict):
        status = response.get("status_code")
    return status in (None, 200)


def _load_json_url(url: str, timeout_sec: int = 60, insecure: bool = False) -> Any:
    req = urllib.request.Request(url, headers={"User-Agent": "video-cut-software/1.0"})
    context: ssl.SSLContext | None = None
    if insecure:
        context = ssl._create_unverified_context()
    else:
        # Prefer certifi CA bundle to avoid local OpenSSL CA mismatch on macOS custom Python.
        try:
            import certifi

            context = ssl.create_default_context(cafile=certifi.where())
        except Exception:
            context = ssl.create_default_context()

    try:
        with urllib.request.urlopen(req, timeout=timeout_sec, context=context) as resp:
            charset = resp.headers.get_content_charset() or "utf-8"
            payload = resp.read().decode(charset, errors="replace")
        return json.loads(payload)
    except urllib.error.URLError as exc:
        raise RuntimeError(
            "下载转写结果失败（可能是本机证书链问题）。"
            "可先执行 `pip install certifi`，或临时加 `--insecure` 跳过证书校验。"
            f" url={url} err={exc}"
        ) from exc


def _iter_sentence_like_nodes(raw: Any):
    if isinstance(raw, dict):
        if isinstance(raw.get("sentences"), list):
            for item in raw["sentences"]:
                yield item

        # DashScope FunASR commonly returns sentence list under transcripts[].
        if isinstance(raw.get("transcripts"), list):
            for tr in raw["transcripts"]:
                if isinstance(tr, dict) and isinstance(tr.get("sentences"), list):
                    for item in tr["sentences"]:
                        yield item

        # Some outputs may nest sentence list by channels/tracks.
        if isinstance(raw.get("channels"), list):
            for ch in raw["channels"]:
                if isinstance(ch, dict) and isinstance(ch.get("sentences"), list):
                    for item in ch["sentences"]:
                        yield item

        if isinstance(raw.get("results"), list):
            for r in raw["results"]:
                yield from _iter_sentence_like_nodes(r)

    elif isinstance(raw, list):
        for item in raw:
            yield from _iter_sentence_like_nodes(item)


def _normalize_text(text: str, strip_ending_punct: bool) -> str:
    t = text.strip()
    if strip_ending_punct:
        # Remove trailing CJK/ASCII punctuation to avoid each subtitle ending with "。"
        t = re.sub(r"[。！？!?；;，,、：:…~]+$", "", t).strip()
    return t


def _to_burn_json(raw_result: Any, language: str, strip_ending_punct: bool) -> dict[str, Any]:
    segments: list[dict[str, Any]] = []
    for s in _iter_sentence_like_nodes(raw_result):
        if not isinstance(s, dict):
            continue
        text = _normalize_text(str(s.get("text", "")), strip_ending_punct=strip_ending_punct)
        if not text:
            continue
        # Skip punctuation-only fragments.
        if re.fullmatch(r"[。！？!?；;，,、：:…~]+", text):
            continue

        begin_ms = s.get("begin_time", s.get("start_time", 0))
        end_ms = s.get("end_time", s.get("stop_time", begin_ms))

        try:
            start = max(0.0, float(begin_ms) / 1000.0)
            end = max(start, float(end_ms) / 1000.0)
        except (TypeError, ValueError):
            continue

        segments.append(
            {
                "start": round(start, 3),
                "end": round(end, 3),
                "text": text,
            }
        )

    segments.sort(key=lambda x: (x["start"], x["end"]))
    return {"language": language, "segments": segments}


def transcribe_with_dashscope(
    file_url: str,
    model: str,
    language: str,
    api_key: str | None,
    poll_sec: float,
    timeout_sec: int,
    insecure: bool,
) -> tuple[dict[str, Any], dict[str, Any]]:
    try:
        import dashscope
        from dashscope.audio.asr import Transcription
    except Exception as exc:
        raise RuntimeError(
            "缺少 dashscope 依赖，请先执行: pip install -U dashscope"
        ) from exc

    if api_key:
        dashscope.api_key = api_key

    # Submit async task.
    resp = Transcription.async_call(
        model=model,
        file_urls=[file_url],
        language_hints=[language],
    )
    if not _response_ok(resp):
        raise RuntimeError(f"任务提交失败: {getattr(resp, 'message', resp)}")

    output = _extract_output(resp)
    task_id = output.get("task_id")
    if not task_id:
        raise RuntimeError(f"提交成功但未返回 task_id，响应: {resp}")

    print(f"task_id: {task_id}", flush=True)

    started = time.time()
    while True:
        result = Transcription.fetch(task=task_id)
        out = _extract_output(result)
        status = out.get("task_status", "UNKNOWN")
        print(f"task_status: {status}", flush=True)

        if status == "SUCCEEDED":
            results = out.get("results") or []
            if not results:
                raise RuntimeError(f"任务成功但未返回 results，output={out}")

            first = results[0] if isinstance(results, list) and results else {}
            transcription_url = first.get("transcription_url") if isinstance(first, dict) else None
            if not transcription_url:
                raise RuntimeError(f"未获取到 transcription_url，results={results}")

            raw_json = _load_json_url(
                transcription_url,
                timeout_sec=timeout_sec,
                insecure=insecure,
            )
            return out, raw_json

        if status == "FAILED":
            raise RuntimeError(f"任务失败，output={out}")

        if time.time() - started > timeout_sec:
            raise TimeoutError(f"等待任务超时（>{timeout_sec}s），最后状态={status}")

        time.sleep(max(0.1, poll_sec))


def main() -> int:
    parser = argparse.ArgumentParser(description="阿里云 FunASR 转写并输出烧录字幕 JSON")
    parser.add_argument("--file-url", default="", help="公网可访问音频 URL（http/https）")
    parser.add_argument("--from-raw-json", default=None, help="从已保存的 raw 结果 JSON 直接转换，不发起新任务")
    parser.add_argument("--output", default="scripts/funasr_burn_script.json", help="烧录 JSON 输出路径")
    parser.add_argument("--raw-output", default="scripts/funasr_raw_result.json", help="原始转写 JSON 输出路径")
    parser.add_argument("--task-output", default="scripts/funasr_task_output.json", help="任务查询 output 输出路径")
    parser.add_argument("--model", default="fun-asr", help="模型名，默认 fun-asr")
    parser.add_argument("--language", default="zh", help="language_hints 首语言，默认 zh")
    parser.add_argument("--api-key", default=os.getenv("DASHSCOPE_API_KEY"), help="DashScope API Key（默认取 DASHSCOPE_API_KEY）")
    parser.add_argument("--poll-sec", type=float, default=1.0, help="轮询间隔秒")
    parser.add_argument("--timeout-sec", type=int, default=1800, help="任务总超时秒数")
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="跳过下载转写结果时的 HTTPS 证书校验（仅用于排查环境问题）",
    )
    parser.add_argument(
        "--keep-ending-punct",
        action="store_true",
        help="保留每段末尾标点（默认会去掉，如句末 。！？）",
    )
    args = parser.parse_args()

    if not args.from_raw_json and not args.file_url.startswith(("http://", "https://")):
        print("--file-url 必须是 http/https 公网 URL", file=sys.stderr)
        return 2

    if not args.from_raw_json and not args.api_key:
        print("缺少 API Key，请设置 --api-key 或环境变量 DASHSCOPE_API_KEY", file=sys.stderr)
        return 2

    output_path = Path(args.output).expanduser().resolve()
    raw_output_path = Path(args.raw_output).expanduser().resolve()
    task_output_path = Path(args.task_output).expanduser().resolve()

    output_path.parent.mkdir(parents=True, exist_ok=True)
    raw_output_path.parent.mkdir(parents=True, exist_ok=True)
    task_output_path.parent.mkdir(parents=True, exist_ok=True)

    if args.from_raw_json:
        raw_input_path = Path(args.from_raw_json).expanduser().resolve()
        if not raw_input_path.exists():
            print(f"--from-raw-json 文件不存在: {raw_input_path}", file=sys.stderr)
            return 2
        raw_result = json.loads(raw_input_path.read_text(encoding="utf-8"))
        task_output = {"mode": "from_raw_json", "raw_json_path": str(raw_input_path)}
    else:
        task_output, raw_result = transcribe_with_dashscope(
            file_url=args.file_url,
            model=args.model,
            language=args.language,
            api_key=args.api_key,
            poll_sec=args.poll_sec,
            timeout_sec=args.timeout_sec,
            insecure=args.insecure,
        )

    burn = _to_burn_json(
        raw_result,
        language=args.language,
        strip_ending_punct=not args.keep_ending_punct,
    )

    task_output_path.write_text(json.dumps(task_output, ensure_ascii=False, indent=2), encoding="utf-8")
    raw_output_path.write_text(json.dumps(raw_result, ensure_ascii=False, indent=2), encoding="utf-8")
    output_path.write_text(json.dumps(burn, ensure_ascii=False, indent=2), encoding="utf-8")

    print(f"burn_json: {output_path}")
    print(f"raw_json: {raw_output_path}")
    print(f"task_output: {task_output_path}")
    print(f"segments: {len(burn.get('segments', []))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
