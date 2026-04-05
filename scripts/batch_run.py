#!/usr/bin/env python3
"""
SiliconFlow 字幕校正客户端 - 并发滑动窗口版（修复全覆盖版）
- 20段/批，返回全部20段（全覆盖，无重叠）
- FLOW.md 全量投入
- 并发执行
"""

import asyncio
import json
import os
import re
import sys
import time
from typing import Optional
from dataclasses import dataclass

try:
    import httpx
except Exception:
    print("ERROR: missing dependency httpx. Install with: pip install httpx", file=sys.stderr)
    raise


API_URL = os.getenv("VCS_API_URL", "https://api.siliconflow.cn/v1/chat/completions")
FLOW_PATH = os.getenv("VCS_FLOW_PATH", "/Users/pxy/PycharmProjects/oauth_login_multi_project/FLOW.md")
ASR_PATH = os.getenv("VCS_ASR_PATH", "/Users/pxy/PycharmProjects/video_cut_software/crates/video_engine/tests/data/full_pipeline_asr.json")
MODEL = os.getenv("VCS_MODEL", "Pro/Qwen/Qwen2.5-7B-Instruct")

# 窗口配置 - 修复：全覆盖，无重叠，返回全部
WINDOW_SIZE = int(os.getenv("VCS_WINDOW_SIZE", "20"))      # 每批20段（减少批量提高稳定性）
RETURN_START = 0      # 返回全部
RETURN_END = WINDOW_SIZE       # 返回全部
OVERLAP = WINDOW_SIZE          # 无重叠，全覆盖

# 并发配置
CONCURRENCY = int(os.getenv("VCS_CONCURRENCY", "30"))       # 并发数


@dataclass
class BatchTask:
    batch_num: int
    window_start: int
    window_end: int
    segments: list[dict]


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8") as f:
        return f.read()


def build_payload(flow_text: str, window_segments: list[dict], window_start_idx: int) -> dict:
    """构建单个窗口的payload"""
    lines = []
    for i, seg in enumerate(window_segments):
        global_idx = window_start_idx + i
        start = float(seg.get('start', 0.0))
        end = float(seg.get('end', 0.0))
        text = str(seg.get('text', '')).strip()
        lines.append(f"[{global_idx}] ({start:.2f}-{end:.2f}) {text}")

    return_start_global = window_start_idx + RETURN_START
    return_end_global = window_start_idx + min(RETURN_END, len(window_segments))

    system_prompt = f"""你是字幕校正助手。任务：修正ASR字幕中的错别字、口语冗余和明显语病。

规则：
1. 不得改变原意，不得新增事实
2. 不得修改时间轴
3. 只修正明显错误（如"俄罗斯"→"OAuth"、"灯路"→"登录"、"练电"→"授权"），保留口语风格
4. 未修改的条目不要返回
5. corrected_text 只返回修正后的纯文本，不要包含时间戳

当前批次：索引 {window_start_idx}-{window_start_idx + len(window_segments) - 1} 共 {len(window_segments)} 条
请只返回索引 {return_start_global}-{return_end_global - 1} 中有修改的校正结果。

输出格式（JSON）：
{{"items":[{{"index":<全局索引>,"corrected_text":"..."}},...]}}"""

    user_content = f"""【参考稿件全文（FLOW.md）】：
{flow_text}

【当前批次字幕】（共 {len(window_segments)} 条）：
{chr(10).join(lines)}

请输出JSON格式的校正结果（仅返回有修改的条目）："""

    return {
        "model": MODEL,
        "temperature": 0.1,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_content},
        ],
    }


async def send_request(client: httpx.AsyncClient, api_key: str, payload: dict) -> tuple[int, str, float]:
    """发送单个请求"""
    t0 = time.perf_counter()
    resp = await client.post(
        API_URL,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        json=payload,
    )
    t1 = time.perf_counter()
    return resp.status_code, resp.text, t1 - t0


def parse_response(resp_text: str, valid_range: range) -> list[dict]:
    """解析响应，返回所有条目"""
    try:
        data = json.loads(resp_text)
        if "choices" in data and len(data["choices"]) > 0:
            content = data["choices"][0]["message"]["content"]

            # 提取 JSON 块
            json_str = None
            if "```json" in content:
                json_str = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                parts = content.split("```")
                if len(parts) >= 3:
                    json_str = parts[1]
            else:
                start = content.find("{")
                end = content.rfind("}")
                if start != -1 and end != -1 and end > start:
                    json_str = content[start:end + 1]

            if not json_str:
                return []

            json_str = json_str.strip()
            open_braces = json_str.count("{")
            close_braces = json_str.count("}")
            if open_braces > close_braces:
                json_str += "}" * (open_braces - close_braces)

            try:
                result = json.loads(json_str)
            except json.JSONDecodeError:
                json_str = re.sub(r'[^}]*$', '', json_str) + "}"
                result = json.loads(json_str)

            items = result.get("items", [])

            # 处理所有条目
            cleaned_items = []
            for item in items:
                if not isinstance(item, dict):
                    continue
                idx = int(item.get("index", -1))
                # 使用 valid_range 过滤
                if idx not in valid_range:
                    continue

                text = item.get("corrected_text", "")
                # 清理时间戳前缀
                if text and text != "null":
                    text = re.sub(r"^\(\d+\.\d+-\d+\.\d+\)\s*", "", text)
                    text = text.strip()
                    if text == "":
                        text = None
                else:
                    text = None

                cleaned_items.append({
                    "index": idx,
                    "corrected_text": text
                })

            return cleaned_items

    except Exception as e:
        print(f"Parse error: {e}, raw: {resp_text[:500]}", file=sys.stderr)
    return []


async def process_batch(
        client: httpx.AsyncClient,
        api_key: str,
        flow_text: str,
        task: BatchTask,
        semaphore: asyncio.Semaphore
) -> tuple[int, list[dict]]:
    """处理单个批次，返回所有条目"""
    async with semaphore:
        window_start = task.window_start
        window = task.segments

        actual_return_start = window_start + RETURN_START
        actual_return_end = window_start + min(RETURN_END, len(window))
        valid_range = range(actual_return_start, actual_return_end)

        print(f"[Batch {task.batch_num}] 处理索引 {window_start}-{task.window_end - 1} (共 {len(window)} 条)",
              file=sys.stderr, flush=True)

        payload = build_payload(flow_text, window, window_start)

        max_retries = 3
        for attempt in range(max_retries):
            try:
                status, resp_text, duration = await send_request(client, api_key, payload)
                if status == 200:
                    items = parse_response(resp_text, valid_range)

                    # 检查缺失的索引，用 null 补全
                    returned_indices = {item["index"] for item in items}
                    for idx in valid_range:
                        if idx not in returned_indices:
                            items.append({
                                "index": idx,
                                "corrected_text": None
                            })

                    items.sort(key=lambda x: x["index"])

                    modified_count = sum(1 for item in items if item["corrected_text"] is not None)
                    print(f"  -> 成功，返回 {len(items)} 条（修改 {modified_count} 条），耗时 {duration:.2f}s",
                          file=sys.stderr, flush=True)
                    return (task.batch_num, items)
                else:
                    print(f"  -> HTTP {status}, 重试 {attempt + 1}/{max_retries}", file=sys.stderr, flush=True)
                    if attempt < max_retries - 1:
                        await asyncio.sleep(1 * (attempt + 1))
            except Exception as e:
                print(f"  -> 错误: {e}, 重试 {attempt + 1}/{max_retries}", file=sys.stderr, flush=True)
                if attempt < max_retries - 1:
                    await asyncio.sleep(1 * (attempt + 1))

        # 全部失败，返回 null
        fallback_items = [
            {"index": idx, "corrected_text": None}
            for idx in valid_range
        ]
        return (task.batch_num, fallback_items)


async def process_all(api_key: str, flow_text: str, segments: list[dict]) -> list[dict]:
    """并发处理所有批次"""
    total = len(segments)

    # 构建批次任务
    tasks = []
    window_start = 0
    batch_num = 0

    while window_start < total:
        batch_num += 1
        end = min(window_start + WINDOW_SIZE, total)
        window = segments[window_start:end]

        tasks.append(BatchTask(
            batch_num=batch_num,
            window_start=window_start,
            window_end=end,
            segments=window
        ))

        window_start += OVERLAP

    print(f"\n共 {len(tasks)} 个批次，并发数 {CONCURRENCY}", file=sys.stderr, flush=True)
    print(f"覆盖索引: 0-{total - 1} (共 {total} 条)", file=sys.stderr, flush=True)
    print(f"预计处理: {total} 条字幕\n", file=sys.stderr, flush=True)

    semaphore = asyncio.Semaphore(CONCURRENCY)

    async with httpx.AsyncClient(timeout=120, verify=True) as client:
        coroutines = [
            process_batch(client, api_key, flow_text, task, semaphore)
            for task in tasks
        ]

        results_list = await asyncio.gather(*coroutines, return_exceptions=True)

    # 合并结果
    all_results = []
    failed_batches = []

    for result in results_list:
        if isinstance(result, Exception):
            print(f"批次异常: {result}", file=sys.stderr)
        else:
            batch_num, items = result
            if items:
                all_results.extend(items)
            else:
                failed_batches.append(batch_num)

    if failed_batches:
        print(f"\n警告: 以下批次处理失败: {failed_batches}", file=sys.stderr)

    return all_results


async def main() -> int:
    api_key = os.getenv("SILICONFLOW_API_KEY")
    if not api_key:
        print("ERROR: SILICONFLOW_API_KEY is not set", file=sys.stderr)
        return 1

    if FLOW_PATH and os.path.exists(FLOW_PATH):
        flow_raw = read_text(FLOW_PATH)
        flow_text = re.sub(r"sk-[A-Za-z0-9_-]+", "[REDACTED_API_KEY]", flow_raw)
    else:
        flow_text = ""

    asr_data = json.loads(read_text(ASR_PATH))
    segments = asr_data.get("segments", [])

    print("=" * 60, file=sys.stderr)
    print(f"开始处理: FLOW {len(flow_text)} 字符, ASR {len(segments)} 段", file=sys.stderr)
    print(f"窗口配置: {WINDOW_SIZE}段/批, 返回{RETURN_START}-{RETURN_END - 1}, 重叠{OVERLAP}段", file=sys.stderr)
    print(f"并发数: {CONCURRENCY}", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    t0 = time.perf_counter()
    results = await process_all(api_key, flow_text, segments)
    t1 = time.perf_counter()

    # 构建最终输出：corrected_text 为 None 的使用原文
    final_items = []
    for item in sorted(results, key=lambda x: x.get("index", 0)):
        idx = item.get("index", 0)
        corrected = item.get("corrected_text")

        # 如果 corrected_text 为 None，使用原文
        if corrected is None:
            if 0 <= idx < len(segments):
                corrected = segments[idx].get('text', '').strip()

        final_items.append({
            "index": idx,
            "corrected_text": corrected
        })

    # 去重（按 index，保留最后一个）
    seen = {}
    for item in final_items:
        seen[item["index"]] = item
    unique_results = list(seen.values())

    # 统计实际修改数量
    modified_count = 0
    for item in unique_results:
        idx = item["index"]
        original = segments[idx].get('text', '').strip() if 0 <= idx < len(segments) else ""
        if original != item["corrected_text"]:
            modified_count += 1

    print("\n" + "=" * 60, file=sys.stderr)
    print(f"处理完成: 总耗时 {t1 - t0:.2f}s", file=sys.stderr)
    print(f"总条目: {len(unique_results)} 条", file=sys.stderr)
    print(f"实际修改: {modified_count} 条", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    unique_results.sort(key=lambda x: x.get("index", 0))
    output = {"items": unique_results}

    json_output = json.dumps(output, ensure_ascii=False, indent=2)
    print(json_output)
    sys.stdout.flush()

    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
