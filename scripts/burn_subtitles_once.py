#!/usr/bin/env python3
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path


def pick_ffmpeg() -> str:
    for p in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"]:
        if Path(p).exists():
            return p
    return "ffmpeg"


def sec_to_srt(sec: float) -> str:
    total_ms = max(0, round(sec * 1000))
    h = total_ms // 3_600_000
    m = (total_ms % 3_600_000) // 60_000
    s = (total_ms % 60_000) // 1000
    ms = total_ms % 1000
    return f"{h:02}:{m:02}:{s:02},{ms:03}"


def track_json_to_srt(subtitle_json: Path, srt_path: Path) -> None:
    data = json.loads(subtitle_json.read_text(encoding="utf-8"))
    segments = data.get("segments", [])
    lines: list[str] = []
    for i, seg in enumerate(segments, start=1):
        start = float(seg.get("start", 0.0))
        end = float(seg.get("end", 0.0))
        text = str(seg.get("text", "")).strip()
        lines.append(str(i))
        lines.append(f"{sec_to_srt(start)} --> {sec_to_srt(end)}")
        lines.append(text)
        lines.append("")
    srt_path.write_text("\n".join(lines), encoding="utf-8")


def find_subtitle_from_project(video_path: Path) -> Path | None:
    downloads = Path.home() / "Downloads"
    if not downloads.exists():
        return None
    projects = sorted(downloads.glob("*.project.json"), key=lambda p: p.stat().st_mtime, reverse=True)
    for project in projects:
        try:
            data = json.loads(project.read_text(encoding="utf-8"))
        except Exception:
            continue
        input_video = Path(str(data.get("inputVideo", ""))).expanduser()
        if input_video.name != video_path.name:
            continue
        for key in ("correctedJson", "subtitlesJson"):
            candidate = Path(str(data.get(key, ""))).expanduser()
            if candidate.exists():
                return candidate
    return None


def detect_subtitle_path(video_path: Path, cli_subtitle: str | None) -> Path:
    if cli_subtitle:
        p = Path(cli_subtitle).expanduser().resolve()
        if not p.exists():
            raise FileNotFoundError(f"字幕文件不存在: {p}")
        return p

    stem = video_path.stem
    candidates = [
        video_path.with_suffix(".asr.corrected.json"),
        video_path.with_suffix(".asr.json"),
        video_path.parent / f"{stem}.asr.corrected.json",
        video_path.parent / f"{stem}.asr.json",
        Path.home() / "Downloads" / f"{stem}.asr.corrected.json",
        Path.home() / "Downloads" / f"{stem}.asr.json",
    ]
    for c in candidates:
        if c.exists():
            return c.resolve()

    from_project = find_subtitle_from_project(video_path)
    if from_project:
        return from_project.resolve()
    raise FileNotFoundError("未找到字幕文件，请通过 --subtitles 指定 .json 或 .srt")


def prepare_srt(subtitles: Path) -> Path:
    tmp_srt = Path(tempfile.gettempdir()) / f"video_cut_burn_{int(time.time())}.srt"
    suffix = subtitles.suffix.lower()
    if suffix == ".srt":
        shutil.copyfile(subtitles, tmp_srt)
        return tmp_srt
    if suffix == ".json":
        track_json_to_srt(subtitles, tmp_srt)
        return tmp_srt
    raise ValueError(f"不支持的字幕格式: {subtitles}")


def escape_for_subtitles_filter(path: Path) -> str:
    s = str(path)
    s = s.replace("\\", "\\\\")
    s = s.replace(":", "\\:")
    s = s.replace("'", "\\'")
    s = s.replace(",", "\\,")
    s = s.replace("[", "\\[")
    s = s.replace("]", "\\]")
    return s


def encoder_args(encoder: str) -> list[str]:
    if encoder == "h264_videotoolbox":
        return ["-c:v", "h264_videotoolbox", "-b:v", "6M"]
    if encoder == "libx264":
        return ["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"]
    raise ValueError(f"不支持的编码器: {encoder}")


def stream_ffmpeg(cmd: list[str]) -> int:
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    assert proc.stdout is not None
    last_progress = time.time()
    for line in proc.stdout:
        text = line.rstrip()
        if "time=" in text or "speed=" in text or "frame=" in text:
            print(text, flush=True)
            last_progress = time.time()
        elif "Error" in text or "error" in text:
            print(text, flush=True)
        elif time.time() - last_progress > 10:
            print("处理中...", flush=True)
            last_progress = time.time()
    return proc.wait()


def burn(video: Path, subtitles: Path, output: Path, font_size: int, encoder: str) -> None:
    ffmpeg = pick_ffmpeg()
    print(f"[1/4] ffmpeg: {ffmpeg}", flush=True)
    print(f"[2/4] 视频: {video}", flush=True)
    print(f"[3/4] 字幕(srt): {subtitles}", flush=True)
    print(f"[4/4] 输出: {output}", flush=True)
    style = (
        f"Alignment=2,FontSize={font_size},"
        "PrimaryColour=&H00E2FF,BackColour=&H000000,BorderStyle=3,Outline=1,Shadow=0"
    )
    vf = f"subtitles={escape_for_subtitles_filter(subtitles)}:force_style='{style}'"
    candidates = [encoder] if encoder != "auto" else ["h264_videotoolbox", "libx264"]
    print("开始烧录，正在输出 ffmpeg 进度...", flush=True)
    last_code = 1
    for idx, enc in enumerate(candidates, start=1):
        print(f"尝试编码器 [{idx}/{len(candidates)}]: {enc}", flush=True)
        cmd = [
            ffmpeg,
            "-y",
            "-i",
            str(video),
            "-vf",
            vf,
            *encoder_args(enc),
            "-c:a",
            "copy",
            str(output),
        ]
        last_code = stream_ffmpeg(cmd)
        if last_code == 0:
            print(f"编码器 {enc} 成功", flush=True)
            break
        print(f"编码器 {enc} 失败，退出码={last_code}", flush=True)
    if last_code != 0:
        raise RuntimeError(f"ffmpeg 失败，退出码={last_code}")
    print("烧录完成", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="给视频烧录字幕（json/srt）")
    parser.add_argument(
        "--video",
        default="/Users/pxy/Desktop/录屏2026-04-05 15.55.27.mov",
        help="视频路径",
    )
    parser.add_argument("--subtitles", default=None, help="字幕路径（.json 或 .srt）")
    parser.add_argument("--font-size", type=int, default=17, help="字幕字号")
    parser.add_argument(
        "--encoder",
        default="auto",
        choices=["auto", "h264_videotoolbox", "libx264"],
        help="视频编码器，auto 会优先硬件编码",
    )
    parser.add_argument("--output", default=None, help="输出视频路径，不填默认写入 scripts")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent
    video_path = Path(args.video).expanduser().resolve()
    if not video_path.exists():
        print(f"视频不存在: {video_path}", file=sys.stderr)
        return 1

    print("解析输入参数完成", flush=True)
    subtitle_path = detect_subtitle_path(video_path, args.subtitles)
    print(f"字幕源: {subtitle_path}", flush=True)
    output_path = (
        Path(args.output).expanduser().resolve()
        if args.output
        else root / f"{video_path.stem}.burned.{int(time.time())}.mp4"
    )

    print("准备字幕文件...", flush=True)
    srt_path = prepare_srt(subtitle_path)
    try:
        burn(video_path, srt_path, output_path, max(12, args.font_size), args.encoder)
    finally:
        if srt_path.exists():
            srt_path.unlink(missing_ok=True)

    print(f"视频: {video_path}")
    print(f"字幕: {subtitle_path}")
    print(f"输出: {output_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print("执行失败，堆栈如下：", file=sys.stderr, flush=True)
        traceback.print_exc()
        raise
