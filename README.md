# 视频剪辑软件 (Video Cut Studio)

本地优先的 macOS 视频处理工具，支持视频剪辑 + 字幕生成全流程：
- 本地视频剪辑（`ffmpeg`）
- 本地音频提取（`ffmpeg`）
- 本地语音识别（`whisper.cpp`）
- AI 字幕校正（阿里百炼 DashScope 兼容 OpenAI 接口）

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
# 编辑 .env 文件，填入有效的 DASHSCOPE_API_KEY
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

## 使用说明

### 前置准备

1. **配置 API Key**：首次使用需要在项目根目录创建 `.env` 文件：
   ```bash
   cp .env.example .env
   # 编辑 .env，填入 DASHSCOPE_API_KEY（用于 AI 字幕校正）
   ```

2. **安装转写引擎**：点击左侧"一键安装转写引擎"按钮，自动安装 whisper-cpp 和语音模型（约 500MB）

---

### 方式一：自动生成字幕（推荐）

**适用场景**：新视频，需要自动生成字幕

**操作流程**：

1. **导入视频**
   - 点击"导入视频"选择视频文件
   - 自动在同目录生成项目文件（.project.json）

2. **（可选）导入参考文本**
   - 如果有演讲稿或剧本，点击"导入参考稿件"
   - AI 校正时会参考文本内容，提高准确率

3. **设置字幕样式**
   - 切换到"样式"标签页
   - 位置：底部/中部/顶部
   - 字号：默认 17，可根据视频调整
   - 颜色：文字颜色 + 背景颜色
   - 圆角：勾选"必须圆角"，调整圆角半径（推荐 16）
   - 内边距：调整文字与背景边缘的距离
   - 留白系数：调整背景宽度（0.5-1.0，越小背景越贴近文字）

4. **生成字幕**
   - 点击"1. 生成字幕"按钮
   - 等待处理：提取音频 → 语音识别 → AI 纠错（约 1-5 分钟，取决于视频长度）
   - 完成后自动加载字幕列表

5. **预览和编辑（可选）**
   - 在字幕列表中点击某行，视频自动跳转到对应时间点
   - 可直接修改文字内容、调整时间戳
   - 点击"保存当前字幕"保存修改

6. **烧录导出**
   - 点击"2. 手动烧录字幕"
   - 字幕将烧录到视频画面
   - 导出文件：`原视频名.subtitled.mp4`

---

### 方式二：加载外部字幕

**适用场景**：已有字幕文件（如从剪映导出），只需要调整样式并烧录

**操作流程**：

1. **导入视频**
   - 点击"导入视频"选择视频文件

2. **加载外部字幕**
   - 切换到"字幕"标签页
   - 点击"加载外部字幕"
   - 选择 JSON 格式的字幕文件

3. **设置样式**
   - 同方式一，调整位置、颜色、圆角等

4. **预览调整（可选）**
   - 检查字幕与画面对齐情况
   - 编辑文字或时间戳

5. **烧录导出**
   - 点击"2. 手动烧录字幕"
   - 等待处理完成

---

### 其他功能

#### 剥离音频
- 点击"3. 剥离音频文件"
- 自动提取视频中的音频，保存为同名 `.wav` 文件到视频所在目录
- 适用于：需要单独处理音频、上传配音平台等场景

#### 保存/加载项目
- **保存项目**：设置好参数后，输入项目路径（默认在视频同目录），点击"保存项目"
- **加载项目**：输入项目路径，点击"加载项目"，可恢复之前的所有设置和字幕

#### 切换字幕版本
- 在"字幕"标签页有三个按钮：
  - **加载识别字幕**：查看 AI 识别原始结果
  - **加载纠正字幕**：查看 AI 纠错后的结果
  - **加载外部字幕**：导入第三方字幕

---

### 使用技巧

1. **字幕太长自动换行**：如果单行字幕过长，ASS 会自动换行，背景框高度会自动适应

2. **快速定位**：在字幕列表点击某行，视频会跳转到该字幕的时间点

3. **批量修改样式**：所有样式修改实时预览，确认满意后再烧录

4. **保留原视频**：烧录后的视频会添加 `.subtitled` 后缀，不会覆盖原视频

---

### 圆角字幕说明

- 勾选"必须圆角（新路线烧录）"可启用圆角背景
- 圆角半径、内边距、留白系数可在"样式"标签页调整
- 文字宽度会自动根据内容估算
- 如果背景太宽或太窄，调节"留白系数"（0.5-1.0）
