# video_cut_software

Local-first macOS app for video cut + subtitle generation pipeline:
- local video cutting (`ffmpeg`)
- local audio extraction (`ffmpeg`)
- local ASR timestamps (`whisper.cpp`)
- subtitle text correction via SiliconFlow text model

## Workspace Structure

- `crates/video_engine`: core processing logic
- `crates/video_cli`: CLI pipeline tools
- `apps/desktop`: Tauri desktop app (runnable)
- `docs/architecture.md`: architecture notes

## Quick Start (Desktop)

1. Prepare env file in project root:

```bash
cp .env.example .env
# edit .env and set a valid SILICONFLOW_API_KEY
```

2. Run desktop dev app:

```bash
cd apps/desktop
pnpm install
pnpm tauri dev
```

3. Build runnable app bundle:

```bash
cd apps/desktop
pnpm run tauri:build
```

Built artifacts:
- `apps/desktop/src-tauri/target/release/bundle/macos/Video Cut Studio.app`

## 常用命令（防忘）

```bash
cd apps/desktop

# 1) 开发模式（看到最新代码）
pnpm tauri dev

# 2) 构建安装版（只打 .app，避免每次都打 dmg）
pnpm run tauri:build

# 3) 一键替换 /Applications 里的安装版并打开
pnpm run mac:refresh-app
```

如果你“关掉再打开还是旧界面”，通常是打开了旧 App。建议固定从这个路径打开：

```bash
open "/Applications/Video Cut Studio.app"
```

## CLI Commands

```bash
source "$HOME/.cargo/env"

cargo run -p video_cli -- cut \
  --input input.mp4 --output clip.mp4 --start 12.5 --duration 8.0

cargo run -p video_cli -- extract-audio \
  --input clip.mp4 --output clip.wav

cargo run -p video_cli -- transcribe \
  --whisper-bin /path/to/whisper-cli \
  --model /path/to/ggml-base.bin \
  --wav clip.wav \
  --whisper-json asr.raw.json \
  --language zh \
  --subtitles-out asr.json

cargo run -p video_cli -- correct-subtitles \
  --subtitles asr.json \
  --output asr.corrected.json \
  --reference script.txt
```
