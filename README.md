# 视频剪辑软件 (Video Cut Studio)

本地优先的 macOS 视频处理工具，支持视频剪辑 + 字幕生成全流程：
- 本地视频剪辑（`ffmpeg`）
- 本地音频提取（`ffmpeg`）
- 本地语音识别（`whisper.cpp`）
- AI 字幕校正（SiliconFlow 大模型）

## 项目结构

- `crates/video_engine`：核心处理逻辑
- `crates/video_cli`：命令行工具
- `apps/desktop`：Tauri 桌面应用（可直接运行）
- `docs/architecture.md`：架构设计文档

## 快速开始（桌面版）

### 1. 配置环境变量

在项目根目录创建 `.env` 文件：

```bash
cp .env.example .env
# 编辑 .env 文件，填入有效的 SILICONFLOW_API_KEY
```

### 2. 运行开发版

```bash
cd apps/desktop
pnpm install
pnpm tauri dev
```

### 3. 构建可安装的 App

```bash
cd apps/desktop
pnpm run tauri:build
```

构建完成后，App 位于：
- `apps/desktop/src-tauri/target/release/bundle/macos/Video Cut Studio.app`

## 常用命令（备忘录）

```bash
cd apps/desktop

# 1) 开发模式（实时看到代码修改）
pnpm tauri dev

# 2) 构建安装版（只生成 .app，不打 dmg）
pnpm run tauri:build

# 3) 一键更新 /Applications 里的安装版并打开（推荐）
pnpm run mac:refresh-app
```

**注意**：如果"关掉再打开还是旧界面"，说明打开的是旧版本。建议固定从这个路径打开：

```bash
open "/Applications/Video Cut Studio.app"
```

## 重新编译安装最新版本

当你拉取了最新代码，或者修改了代码后，需要重新编译安装：

```bash
# 进入桌面应用目录
cd /Users/pxy/PycharmProjects/video_cut_software/apps/desktop

# 一键编译并安装到 /Applications
pnpm run mac:refresh-app
```

这个命令会：
1. 编译前端代码
2. 编译 Rust 后端
3. 打包成 .app
4. 复制到 /Applications
5. 自动打开应用

## CLI 命令行工具

如果你更喜欢用命令行：

```bash
# 加载 Cargo 环境
source "$HOME/.cargo/env"

# 1. 视频剪辑
cargo run -p video_cli -- cut \
  --input input.mp4 --output clip.mp4 --start 12.5 --duration 8.0

# 2. 提取音频
cargo run -p video_cli -- extract-audio \
  --input clip.mp4 --output clip.wav

# 3. 剥离音频（自动保存到同目录）
cargo run -p video_cli -- strip-audio --input video.mp4

# 4. 语音识别（需要 whisper-cli）
cargo run -p video_cli -- transcribe \
  --whisper-bin /path/to/whisper-cli \
  --model /path/to/ggml-base.bin \
  --wav clip.wav \
  --whisper-json asr.raw.json \
  --language zh \
  --subtitles-out asr.json

# 5. AI 校正字幕
cargo run -p video_cli -- correct-subtitles \
  --subtitles asr.json \
  --output asr.corrected.json \
  --reference script.txt
```

## 清理编译缓存

如果项目文件夹占用空间太大（几个GB），可以清理编译缓存：

```bash
# 清理所有 Rust 编译输出（可释放 5GB+）
cd /Users/pxy/PycharmProjects/video_cut_software
cargo clean
cd apps/desktop/src-tauri && cargo clean

# 删除测试生成的视频文件
rm -f scripts/*.subtitled.mp4 scripts/*.rounded.test.mp4
```

## 功能说明

### 当前支持的功能

1. **导入视频**：选择本地视频文件
2. **生成字幕**：自动提取音频 → 语音识别 → AI 校正
3. **手动烧录字幕**：将字幕烧录到视频画面
4. **剥离音频**：单独提取视频中的音频文件
5. **字幕样式调整**：位置、字号、颜色、圆角、透明度等

### 圆角字幕说明

- 勾选"必须圆角（新路线烧录）"可启用圆角背景
- 圆角半径、内边距、留白系数可在"样式"标签页调整
- 文字宽度会自动根据内容估算
