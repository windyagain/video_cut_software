#!/usr/bin/env python3
"""
SiliconFlow 流式字幕校正客户端
- 流式接收响应，实时显示进度
- 支持 JSON 流式解析（处理可能的分块问题）
- 优化 Prompt 适应流式输出
"""

import asyncio
import contextlib
import json
import os
import re
import sys
import time
from typing import AsyncIterator, Optional

try:
    import httpx
except Exception:
    print("ERROR: missing dependency httpx. Install with: pip install httpx", file=sys.stderr)
    raise

API_URL = "https://api.siliconflow.cn/v1/chat/completions"
FLOW_PATH = "/Users/pxy/PycharmProjects/oauth_login_multi_project/FLOW.md"
ASR_PATH = "/Users/pxy/PycharmProjects/video_cut_software/crates/video_engine/tests/data/full_pipeline_asr.json"
MODEL = "Pro/zai-org/GLM-5"


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8") as f:
        return f.read()


def build_payload() -> dict:
    """构建流式请求 payload，优化 prompt 适应流式输出"""
    flow = read_text(FLOW_PATH)
    flow = re.sub(r"sk-[A-Za-z0-9_-]+", "[REDACTED_API_KEY]", flow)

    asr = json.loads(read_text(ASR_PATH))
    segments = asr.get("segments", [])

    # 简化为索引映射，减少 token 消耗
    asr_summary = "\n".join([
        f"[{i}] ({float(seg.get('start', 0.0)):.2f}s-{float(seg.get('end', 0.0)):.2f}s): {str(seg.get('text', '')).strip()}"
        for i, seg in enumerate(segments)
    ])

    # 优化后的 Prompt：要求流式输出 JSON Lines 格式，每行一个校正结果
    system_prompt = """你是字幕校正助手。任务：修正 ASR 字幕中的错别字、口语冗余和明显语病。

规则：
1. 不得改变原意，不得新增事实
2. 不得修改时间轴
3. 只输出纯文本校正结果，不要解释

输出格式（JSON Lines，每行一个）：
{"index":0,"corrected_text":"校正后的文本"}
{"index":1,"corrected_text":"校正后的文本"}
...

要求：严格按索引顺序输出，一行一个 JSON 对象。"""

    user_content = f"""请校正以下 ASR 字幕，按索引顺序流式输出每行的校正结果。

参考稿件全文（FLOW.md，已脱敏）：
{flow}

ASR 字幕分段（共 {len(segments)} 段）：
{asr_summary}

请开始输出校正结果（JSON Lines 格式）："""

    return {
        "model": MODEL,
        "temperature": 0.1,
        "stream": True,  # 启用流式
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_content},
        ],
    }


async def stream_request(
        api_key: str, payload: dict, verify: bool
) -> AsyncIterator[tuple[Optional[str], Optional[dict]]]:
    """
    流式发送请求，yield (raw_text_chunk, parsed_json_or_none)
    解析 SSE 格式的流式响应
    """
    async with httpx.AsyncClient(timeout=1200, verify=verify) as client:
        async with client.stream(
                "POST",
                API_URL,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                },
                json=payload,
        ) as response:
            if response.status_code != 200:
                # 非 200 时，读取完整错误信息
                body = await response.aread()
                raise httpx.HTTPStatusError(
                    f"HTTP {response.status_code}",
                    request=response.request,
                    response=httpx.Response(response.status_code, content=body)
                )

            # 解析 SSE 流
            buffer = ""
            async for chunk in response.aiter_text():
                buffer += chunk
                lines = buffer.split("\n")
                buffer = lines.pop()  # 保留不完整的最后一行

                for line in lines:
                    line = line.strip()
                    if line.startswith("data: "):
                        data = line[6:]  # 去掉 "data: " 前缀
                        if data == "[DONE]":
                            return

                        try:
                            event = json.loads(data)
                            # 提取 content delta
                            if event.get("choices"):
                                delta = event["choices"][0].get("delta", {})
                                content = delta.get("content", "")
                                if content:
                                    yield content, None
                        except json.JSONDecodeError:
                            # 忽略解析错误，继续
                            pass


def parse_json_lines(text: str) -> list[dict]:
    """从累积文本中解析 JSON Lines"""
    results = []
    for line in text.strip().split("\n"):
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
            if "index" in obj and "corrected_text" in obj:
                results.append(obj)
        except json.JSONDecodeError:
            continue
    return results


async def main() -> int:
    api_key = os.getenv("SILICONFLOW_API_KEY")
    if not api_key:
        print("ERROR: SILICONFLOW_API_KEY is not set", file=sys.stderr)
        return 1

    payload = build_payload()

    print("=" * 60, file=sys.stderr)
    print("开始流式请求...", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    accumulated = ""
    parsed_items = []
    t0 = time.perf_counter()

    try:
        async for chunk, _ in stream_request(api_key, payload, verify=True):
            if chunk:
                accumulated += chunk
                # 实时打印流式输出（可选：显示进度）
                print(chunk, end="", flush=True)

                # 尝试解析已累积的完整行
                # 简单启发式：如果包含换行符，尝试解析
                if "\n" in accumulated:
                    new_items = parse_json_lines(accumulated)
                    if len(new_items) > len(parsed_items):
                        parsed_items = new_items

    except httpx.ConnectError as e:
        if "CERTIFICATE_VERIFY_FAILED" in str(e):
            print("\nWARN: certificate verify failed, retry with verify=False", file=sys.stderr)
            try:
                async for chunk, _ in stream_request(api_key, payload, verify=False):
                    if chunk:
                        accumulated += chunk
                        print(chunk, end="", flush=True)
            except Exception as e2:
                print(f"\nREQUEST_ERROR: {e2}", file=sys.stderr)
                return 2
        else:
            print(f"\nREQUEST_ERROR: {e}", file=sys.stderr)
            return 2
    except httpx.HTTPStatusError as e:
        print(f"\nHTTP_ERROR: {e}", file=sys.stderr)
        return 2
    except Exception as e:
        print(f"\nREQUEST_ERROR: {e}", file=sys.stderr)
        return 2

    t1 = time.perf_counter()
    duration = t1 - t0

    print("\n" + "=" * 60, file=sys.stderr)
    print(f"流式接收完成，总耗时: {duration:.3f}s", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    # 最终解析
    final_items = parse_json_lines(accumulated)

    # 输出完整结果
    result = {"items": final_items}
    print(json.dumps(result, ensure_ascii=False, indent=2))

    print(f"\n总计校正条目: {len(final_items)}", file=sys.stderr)
    print(f"原始字符数: {len(accumulated)}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))