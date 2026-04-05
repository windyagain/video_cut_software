#!/usr/bin/env python3
import asyncio
import contextlib
import json
import os
import re
import sys
import time

try:
    import httpx
except Exception:
    print("ERROR: missing dependency httpx. Install with: pip install httpx", file=sys.stderr)
    raise


API_URL = "https://api.siliconflow.cn/v1/chat/completions"
FLOW_PATH = "/Users/pxy/PycharmProjects/oauth_login_multi_project/FLOW.md"
ASR_PATH = "/Users/pxy/PycharmProjects/video_cut_software/crates/video_engine/tests/data/full_pipeline_asr.json"
MODEL = "Pro/Qwen/Qwen2.5-7B-Instruct"


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8") as f:
        return f.read()


def build_payload() -> dict:
    flow = read_text(FLOW_PATH)
    flow = re.sub(r"sk-[A-Za-z0-9_-]+", "[REDACTED_API_KEY]", flow)

    asr = json.loads(read_text(ASR_PATH))
    segments = asr.get("segments", [])
    lines = [
        f"[{i}] ({float(seg.get('start', 0.0)):.2f}-{float(seg.get('end', 0.0)):.2f}) {str(seg.get('text', '')).strip()}"
        for i, seg in enumerate(segments)
    ]

    return {
        "model": MODEL,
        "temperature": 0.1,
        "messages": [
            {
                "role": "system",
                "content": "你是字幕校正助手。只修正错别字、口语冗余和明显语病；不得改变原意，不得新增事实，不得改时间轴。输出必须是 JSON。",
            },
            {
                "role": "user",
                "content": (
                    "请校正以下 ASR 字幕。\n\n"
                    "输出格式严格为：{\"items\":[{\"index\":0,\"corrected_text\":\"...\"}]}\n"
                    "仅返回 JSON，不要解释。\n\n"
                    "参考稿件全文（FLOW.md，已脱敏）：\n"
                    + flow
                    + "\n\nASR 字幕分段（完整）：\n"
                    + "\n".join(lines)
                ),
            },
        ],
    }


async def send_request(api_key: str, payload: dict, verify: bool) -> tuple[int, str, float]:
    async def heartbeat(stop_event: asyncio.Event, start_ts: float) -> None:
        while not stop_event.is_set():
            elapsed = time.perf_counter() - start_ts
            print(f"REQUESTING... {elapsed:.1f}s", file=sys.stderr, flush=True)
            try:
                await asyncio.wait_for(stop_event.wait(), timeout=1.0)
            except asyncio.TimeoutError:
                continue

    stop_event = asyncio.Event()
    t0 = time.perf_counter()
    hb_task = asyncio.create_task(heartbeat(stop_event, t0))
    try:
        async with httpx.AsyncClient(timeout=1200, verify=verify) as client:
            resp = await client.post(
                API_URL,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                },
                json=payload,
            )
    finally:
        stop_event.set()
        with contextlib.suppress(Exception):
            await hb_task
    t1 = time.perf_counter()
    return resp.status_code, resp.text, t1 - t0


def main() -> int:
    api_key = os.getenv("SILICONFLOW_API_KEY")
    if not api_key:
        print("ERROR: SILICONFLOW_API_KEY is not set", file=sys.stderr)
        return 1

    payload = build_payload()

    try:
        status, resp_body, duration = asyncio.run(send_request(api_key, payload, verify=True))
    except httpx.ConnectError as e:
        # Common on local Python envs with broken CA chain.
        if "CERTIFICATE_VERIFY_FAILED" in str(e):
            print("WARN: certificate verify failed, retry with verify=False", file=sys.stderr)
            try:
                status, resp_body, duration = asyncio.run(
                    send_request(api_key, payload, verify=False)
                )
            except Exception as e2:
                print(f"REQUEST_ERROR: {e2}", file=sys.stderr)
                return 2
        else:
            print(f"REQUEST_ERROR: {e}", file=sys.stderr)
            return 2
    except httpx.HTTPStatusError as e:
        status = e.response.status_code
        resp_body = e.response.text
        duration = 0.0
    except Exception as e:
        print(f"REQUEST_ERROR: {e}", file=sys.stderr)
        return 2

    print(resp_body)
    print()
    print(f"HTTP:{status}")
    print(f"TOTAL:{duration:.3f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
