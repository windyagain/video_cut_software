# Video Cut Software - MVP Architecture

## Goals
- Local video cut and audio extraction via ffmpeg
- Local speech recognition via whisper.cpp (timestamped segments)
- Subtitle text polishing via SiliconFlow LLM
- Basic subtitle style settings (position, background, color)

## Tech Stack
- Core engine: Rust (`video_engine`)
- CLI integration/testing: Rust (`video_cli`)
- Desktop UI (next phase): Tauri + web frontend
- Media processing: ffmpeg
- ASR: whisper.cpp
- Text correction: SiliconFlow Chat Completions API

## Modules
- `video_engine::ffmpeg`: cut video, extract wav
- `video_engine::subtitle`: subtitle models and JSON IO
- `video_engine::siliconflow`: call cloud model to correct subtitle lines
- `video_cli`: thin command wrapper for local workflows

## Processing Pipeline
1. `ffmpeg` cut source video to target clip
2. `ffmpeg` extract mono 16k wav from clip
3. whisper.cpp transcribe audio to segment timestamps (JSON)
4. load segment JSON into subtitle model
5. send segment text + optional reference script to SiliconFlow
6. merge corrected text back without changing timestamps
7. save project JSON

## Why this design
- Performance-sensitive operations stay local
- Timestamp stability comes from local ASR
- Text quality improvement uses existing cloud model account
- Rust modules are reusable for future Tauri UI
