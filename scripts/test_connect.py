#!/usr/bin/env python3
import asyncio
import os
import time

try:
    import httpx
except Exception:
    print("ERROR: 请先安装 httpx: pip install httpx")
    raise

API_URL = "https://api.siliconflow.cn/v1/chat/completions"
MODEL = "Pro/Qwen/Qwen2.5-7B-Instruct"


async def simple_chat():
    api_key = os.getenv("SILICONFLOW_API_KEY")
    if not api_key:
        print("ERROR: 请设置环境变量 SILICONFLOW_API_KEY")
        return 1

    payload = {
        "model": MODEL,
        "temperature": 0.7,
        "messages": [
            {"role": "user", "content": "你好"}
        ]
    }

    print("发送请求: 你好")
    t0 = time.perf_counter()

    async with httpx.AsyncClient(timeout=30) as client:
        resp = await client.post(
            API_URL,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            json=payload,
        )

    t1 = time.perf_counter()

    print(f"\n状态码: {resp.status_code}")
    print(f"耗时: {t1 - t0:.2f}秒")
    print(f"\n回复内容:\n{resp.text}")

    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(simple_chat()))