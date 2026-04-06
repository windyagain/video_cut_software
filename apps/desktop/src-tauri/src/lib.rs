use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{Emitter, Manager};
use video_engine::siliconflow::SiliconFlowClient;
use video_engine::subtitle::SubtitleTrack;
use video_engine::{asr, ffmpeg};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubtitleStyle {
    position: String,
    font_size: u32,
    text_color: String,
    background_color: String,
    #[serde(default)]
    rounded_required: bool,
    #[serde(default = "default_rounded_radius")]
    rounded_radius: u32,
    #[serde(default = "default_box_padding")]
    box_padding: u32,
    #[serde(default = "default_bg_opacity")]
    bg_opacity: u8,
    #[serde(default = "default_x_padding_scale")]
    x_padding_scale: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectConfig {
    input_video: String,
    clipped_video: String,
    #[serde(default)]
    rendered_video: String,
    audio_wav: String,
    #[serde(default)]
    tool_api_origin: String,
    #[serde(default)]
    dashscope_api_key: String,
    #[serde(default)]
    asr_model: String,
    whisper_json: String,
    subtitles_json: String,
    corrected_json: String,
    reference_script: String,
    language: String,
    cut_start: f32,
    cut_duration: f32,
    subtitle_style: SubtitleStyle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalSetup {
    ffmpeg_bin: Option<String>,
    whisper_bin: Option<String>,
    whisper_model: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CorrectionRuntimeConfig {
    model: String,
    batch_size: usize,
    concurrency: usize,
    max_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AsrRuntimeConfig {
    api_origin: String,
    dashscope_api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BatchCorrectedItem {
    index: usize,
    corrected_text: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BatchCorrectedOutput {
    items: Vec<BatchCorrectedItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    route: String,
    output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgressPayload {
    percent: u32,
    text: String,
}

fn first_existing(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|p| Path::new(**p).exists())
        .map(|p| (*p).to_string())
}

fn default_model_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".video_cut_studio")
            .join("models")
            .join("ggml-base.bin"),
    )
}

fn correction_batch_size() -> usize {
    std::env::var("SILICONFLOW_CORRECT_BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
}

fn correction_concurrency() -> usize {
    std::env::var("SILICONFLOW_CORRECT_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
}

fn correction_max_tokens() -> u32 {
    std::env::var("SILICONFLOW_CORRECT_MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(8192)
}

fn correction_model() -> String {
    std::env::var("SILICONFLOW_CORRECT_MODEL")
        .or_else(|_| std::env::var("SILICONFLOW_TEXT_MODEL"))
        .unwrap_or_else(|_| "Pro/Qwen/Qwen2.5-7B-Instruct".to_string())
}

fn asr_api_origin() -> String {
    std::env::var("TOOL_AUDIO_ASR_ORIGIN")
        .or_else(|_| std::env::var("TOOL_API_ORIGIN"))
        .unwrap_or_else(|_| "http://101.34.207.228:81".to_string())
}

fn resolve_dashscope_api_key() -> Option<String> {
    let _ = dotenvy::dotenv();
    let mut paths = Vec::<PathBuf>::new();
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(&home).join(".env"));
        paths.push(PathBuf::from(home).join(".video_cut_studio").join(".env"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.env"));
    for path in paths {
        if path.exists() {
            let _ = dotenvy::from_path(path);
        }
    }
    std::env::var("DASHSCOPE_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn default_download_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Downloads"))
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_ffmpeg_bin() -> String {
    first_existing(&[
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ])
        .or_else(|| std::env::var("FFMPEG_BIN").ok())
        .unwrap_or_else(|| "ffmpeg".to_string())
}

fn sanitize_file_stem(name: &str) -> String {
    let mapped = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = mapped.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "video_cut_project".to_string()
    } else {
        trimmed
    }
}

fn detect_local_setup_impl() -> LocalSetup {
    let ffmpeg_bin = first_existing(&[
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ]);

    let whisper_bin = first_existing(&[
        "/opt/homebrew/bin/whisper-cli",
        "/usr/local/bin/whisper-cli",
        "/opt/homebrew/bin/whisper-cpp",
        "/usr/local/bin/whisper-cpp",
    ]);

    let mut whisper_model = None;
    if let Some(p) = default_model_path() {
        if p.exists() {
            whisper_model = Some(p.to_string_lossy().to_string());
        }
    }

    let message = if whisper_bin.is_some() && whisper_model.is_some() {
        "本地转写环境已就绪".to_string()
    } else if whisper_bin.is_none() && whisper_model.is_none() {
        "未找到 whisper 可执行文件和模型，可点击一键安装".to_string()
    } else if whisper_bin.is_none() {
        "未找到 whisper 可执行文件，可点击一键安装".to_string()
    } else {
        "未找到 whisper 模型，可点击一键安装".to_string()
    };

    LocalSetup {
        ffmpeg_bin,
        whisper_bin,
        whisper_model,
        message,
    }
}

#[tauri::command]
fn detect_local_setup() -> LocalSetup {
    detect_local_setup_impl()
}

#[tauri::command]
fn install_local_whisper_base() -> Result<LocalSetup, String> {
    let brew = first_existing(&["/opt/homebrew/bin/brew", "/usr/local/bin/brew"])
        .ok_or("未找到 Homebrew，请先安装 brew")?;

    let status = Command::new(&brew)
        .arg("list")
        .arg("whisper-cpp")
        .status()
        .map_err(|e| format!("执行 brew list 失败: {e}"))?;
    if !status.success() {
        let output = Command::new(&brew)
            .arg("install")
            .arg("whisper-cpp")
            .output()
            .map_err(|e| format!("安装 whisper-cpp 失败: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "brew install whisper-cpp 失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    let model_path = default_model_path().ok_or("无法解析 HOME 目录")?;
    if let Some(parent) = model_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建模型目录失败: {e}"))?;
    }

    if !model_path.exists() {
        let model_url =
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
        let output = Command::new("curl")
            .arg("-L")
            .arg("-o")
            .arg(&model_path)
            .arg(model_url)
            .output()
            .map_err(|e| format!("下载模型失败: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "下载模型失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    Ok(detect_local_setup_impl())
}

#[tauri::command]
fn pick_video_file() -> Option<String> {
    FileDialog::new()
        .add_filter("video", &["mp4", "mov", "mkv", "avi"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_reference_file() -> Option<String> {
    FileDialog::new()
        .add_filter("text", &["txt", "md"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_subtitle_file() -> Option<String> {
    FileDialog::new()
        .add_filter("subtitle", &["json"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_export_file(suggested_path: Option<String>) -> Option<String> {
    let mut dialog = FileDialog::new().add_filter("video", &["mp4"]);
    if let Some(path) = suggested_path.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(path);
        if let Some(parent) = p.parent() {
            dialog = dialog.set_directory(parent);
        }
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            dialog = dialog.set_file_name(name);
        }
    }
    dialog
        .save_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn suggest_project_path(input_video: Option<String>) -> Result<String, String> {
    let base_name = input_video
        .as_deref()
        .map(Path::new)
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(sanitize_file_stem)
        .unwrap_or_else(|| "video_cut_project".to_string());

    let file_name = format!("{base_name}.project.json");
    let path = default_download_dir().join(file_name);
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_correction_runtime_config() -> CorrectionRuntimeConfig {
    CorrectionRuntimeConfig {
        model: correction_model(),
        batch_size: correction_batch_size(),
        concurrency: correction_concurrency(),
        max_tokens: correction_max_tokens(),
    }
}

#[tauri::command]
fn get_asr_runtime_config() -> AsrRuntimeConfig {
    AsrRuntimeConfig {
        api_origin: asr_api_origin(),
        dashscope_api_key: resolve_dashscope_api_key().unwrap_or_default(),
    }
}

#[tauri::command]
fn cut_video(input: String, output: String, start: f32, duration: f32) -> Result<(), String> {
    ffmpeg::cut_video(input, output, start, duration).map_err(|e| e.to_string())
}

#[tauri::command]
async fn extract_audio(input: String, output: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        ffmpeg::extract_audio_wav_mono16k(input, output).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("extract_audio task join failed: {e}"))?
}

#[tauri::command]
async fn strip_audio(input: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let input_path = Path::new(&input);
        let parent = input_path
            .parent()
            .filter(|p| p.as_os_str().len() > 0)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");
        let output_path = parent.join(format!("{}.wav", stem));
        let output = output_path.to_string_lossy().to_string();
        
        ffmpeg::extract_audio_wav_mono16k(&input, &output).map_err(|e| e.to_string())?;
        Ok(output)
    })
    .await
    .map_err(|e| format!("strip_audio task join failed: {e}"))?
}

#[tauri::command]
async fn transcribe_audio(
    dashscope_api_key: String,
    asr_model: String,
    wav: String,
    whisper_json: String,
    subtitles_out: String,
) -> Result<(), String> {
    let final_api_origin = asr_api_origin();
    let final_api_key = if dashscope_api_key.trim().is_empty() {
        resolve_dashscope_api_key().unwrap_or_default()
    } else {
        dashscope_api_key.trim().to_string()
    };
    if final_api_key.is_empty() {
        return Err("请填写 DASHSCOPE_API_KEY".to_string());
    }
    let (raw_body, track) = asr::transcribe_with_tool_asr(
        &final_api_origin,
        &wav,
        &final_api_key,
        &asr_model,
    )
    .await
    .map_err(|e| e.to_string())?;
    fs::write(&whisper_json, raw_body).map_err(|e| e.to_string())?;
    track.to_json_file(&subtitles_out).map_err(|e| e.to_string())
}

#[tauri::command]
async fn burn_subtitles(
    app: tauri::AppHandle,
    input_video: String,
    subtitles_json: String,
    output_video: String,
    style: SubtitleStyle,
) -> Result<String, String> {
    export_subtitled_video(app, input_video, subtitles_json, output_video, "source".to_string(), "6M".to_string(), style)
        .await
        .map(|r| r.route)
}

fn parse_resolution(resolution: &str) -> Option<(u32, u32)> {
    let r = resolution.trim().to_ascii_lowercase();
    if r.is_empty() || r == "source" {
        return None;
    }
    let mut parts = r.split('x');
    let w = parts.next()?.parse::<u32>().ok()?;
    let h = parts.next()?.parse::<u32>().ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

fn emit_export_progress(app: &tauri::AppHandle, percent: u32, text: &str) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.emit(
            "export-progress",
            ExportProgressPayload {
                percent: percent.min(100),
                text: text.to_string(),
            },
        );
    }
}

fn burn_video_with_progress(
    app: &tauri::AppHandle,
    input_video: &str,
    output_video: &str,
    vf: String,
    video_bitrate: Option<&str>,
) -> Result<(), String> {
    let total_duration = probe_video_duration(input_video).unwrap_or(0.0);
    let ffmpeg_bin = resolve_ffmpeg_bin();
    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.arg("-y")
        .arg("-i")
        .arg(input_video)
        .arg("-vf")
        .arg(vf)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-progress")
        .arg("pipe:2")
        .arg("-nostats");
    if let Some(br) = video_bitrate.filter(|s| !s.trim().is_empty()) {
        cmd.arg("-b:v").arg(br.trim());
    }
    let mut child = cmd
        .arg(output_video)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 ffmpeg 导出失败: {e}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 ffmpeg 导出进度".to_string())?;
    emit_export_progress(app, 2, "开始导出");
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        let line = line.unwrap_or_default();
        if let Some(ms) = line.strip_prefix("out_time_ms=") {
            if let Ok(out_time_ms) = ms.trim().parse::<f64>() {
                if total_duration > 0.0 {
                    let seconds = out_time_ms / 1_000_000.0;
                    let percent = ((seconds / total_duration) * 100.0).round() as u32;
                    emit_export_progress(app, percent.min(99), &format!("导出中 {}%", percent.min(99)));
                }
            }
        } else if line == "progress=end" {
            emit_export_progress(app, 100, "导出完成");
        }
    }
    let status = child.wait().map_err(|e| format!("等待 ffmpeg 结束失败: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg 导出失败: status={status}"));
    }
    emit_export_progress(app, 100, "导出完成");
    Ok(())
}

#[tauri::command]
async fn export_subtitled_video(
    app: tauri::AppHandle,
    input_video: String,
    subtitles_json: String,
    output_video: String,
    resolution: String,
    bitrate: String,
    style: SubtitleStyle,
) -> Result<ExportResult, String> {
    emit_export_progress(&app, 0, "准备导出");
    tauri::async_runtime::spawn_blocking(move || {
        let track = SubtitleTrack::from_json_file(&subtitles_json).map_err(|e| e.to_string())?;
        let target_res = parse_resolution(&resolution);
        let target_bitrate = bitrate.trim().to_string();
        if style.rounded_required {
            let ass_path = std::env::temp_dir().join("video_cut_studio_burn.rounded.ass");
            let ass_content = build_rounded_ass_script(&track, &style, &input_video);
            fs::write(&ass_path, ass_content).map_err(|e| format!("写入ASS文件失败: {e}"))?;
            burn_video_with_progress(
                &app,
                &input_video,
                &output_video,
                format!("subtitles='{}'{}", escape_filter_path(&ass_path), target_res.map(|(w, h)| format!(",scale={w}:{h}")).unwrap_or_default()),
                Some(target_bitrate.as_str()),
            )?;
            Ok(ExportResult {
                route: format!("route=rounded_ass, ass_path={}", ass_path.to_string_lossy()),
                output: output_video,
            })
        } else {
            let srt_path = std::env::temp_dir().join("video_cut_studio_burn.srt");
            track.to_srt_file(&srt_path).map_err(|e| e.to_string())?;
            let force_style = build_ass_style(&style);
            let sub_path = escape_filter_path(&srt_path);
            let escaped_style = force_style.replace('\'', "\\'");
            let mut vf = format!("subtitles='{}':force_style='{}'", sub_path, escaped_style);
            if let Some((w, h)) = target_res {
                vf.push_str(&format!(",scale={w}:{h}"));
            }
            burn_video_with_progress(
                &app,
                &input_video,
                &output_video,
                vf,
                Some(target_bitrate.as_str()),
            )?;
            Ok(ExportResult {
                route: format!("route=legacy_srt, srt_path={}", srt_path.to_string_lossy()),
                output: output_video,
            })
        }
    })
    .await
    .map_err(|e| format!("export_subtitled_video task join failed: {e}"))?
}

#[tauri::command]
async fn correct_subtitles(
    subtitles: String,
    output: String,
    reference: Option<String>,
) -> Result<(), String> {
    let (track, reference_content) =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
            let track = SubtitleTrack::from_json_file(&subtitles).map_err(|e| e.to_string())?;
            let reference_content = reference
                .filter(|s| !s.trim().is_empty())
                .map(fs::read_to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            Ok((track, reference_content))
        })
        .await
        .map_err(|e| format!("correct_subtitles load task join failed: {e}"))??;

    let client = SiliconFlowClient::from_env().map_err(|e| e.to_string())?;
    let corrected = client
        .correct_subtitles(&track, reference_content.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    let mut new_track = track.clone();
    for item in corrected {
        if let Some(seg) = new_track.segments.get_mut(item.index) {
            seg.text = sanitize_corrected_text(&seg.text, &item.corrected_text);
        }
    }

    tauri::async_runtime::spawn_blocking(move || new_track.to_json_file(output).map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("correct_subtitles save task join failed: {e}"))?
}

#[tauri::command]
fn correct_subtitles_batch(
    subtitles: String,
    output: String,
    reference: Option<String>,
) -> Result<String, String> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scripts/batch_run.py");
    if !script.exists() {
        return Err(format!(
            "batch 脚本不存在: {}",
            script.to_string_lossy()
        ));
    }

    let python = first_existing(&[
        "/usr/local/bin/python3.11",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
        "/usr/bin/python3",
    ])
    .ok_or("未找到 python3，请先安装 Python 3.11+")?;

    let mut cmd = Command::new(&python);
    cmd.arg(&script);
    cmd.env("PYTHONUNBUFFERED", "1");
    cmd.env("VCS_ASR_PATH", &subtitles);
    if let Some(r) = reference.filter(|s| !s.trim().is_empty()) {
        cmd.env("VCS_FLOW_PATH", r);
    } else {
        cmd.env("VCS_FLOW_PATH", "");
    }

    let out = cmd
        .output()
        .map_err(|e| format!("执行 batch_run.py 失败: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "batch_run.py 执行失败: status={:?}, stderr={}, stdout={}",
            out.status.code(),
            stderr.trim(),
            stdout.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let json_payload = extract_json_payload(&stdout).unwrap_or(stdout);
    let corrected: BatchCorrectedOutput =
        serde_json::from_str(&json_payload).map_err(|e| format!("解析 batch 输出 JSON 失败: {e}"))?;

    let mut track = SubtitleTrack::from_json_file(&subtitles).map_err(|e| e.to_string())?;
    let mut applied = 0usize;
    for item in corrected.items {
        if let Some(seg) = track.segments.get_mut(item.index) {
            let next_text = sanitize_corrected_text(&seg.text, &item.corrected_text);
            if seg.text != next_text {
                applied += 1;
            }
            seg.text = next_text;
        }
    }

    track.to_json_file(&output).map_err(|e| e.to_string())?;
    Ok(format!(
        "batch纠正完成: 总段数={}, 应用修改={}, 输出={}",
        track.segments.len(),
        applied,
        output
    ))
}

#[tauri::command]
fn load_subtitles(path: String) -> Result<SubtitleTrack, String> {
    SubtitleTrack::from_json_file(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_subtitles(path: String, track: SubtitleTrack) -> Result<(), String> {
    track.to_json_file(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_project(path: String, project: ProjectConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_project(path: String) -> Result<ProjectConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

fn build_ass_style(style: &SubtitleStyle) -> String {
    let align = match style.position.as_str() {
        "top" => 8,
        "center" => 5,
        _ => 2,
    };
    let primary = hex_to_ass(&style.text_color, 0x00);
    let alpha = opacity_percent_to_ass_alpha(style.bg_opacity);
    let back = hex_to_ass(&style.background_color, alpha);
    format!(
        "Alignment={align},FontSize={},PrimaryColour={primary},BackColour={back},BorderStyle=4,Bold=1,Outline=0,Shadow=0,MarginV=24,Spacing=0",
        style.font_size
    )
}

fn normalize_inline_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_corrected_text(original: &str, corrected: &str) -> String {
    let original_n = normalize_inline_text(original);
    let corrected_n = normalize_inline_text(corrected);
    if corrected_n.is_empty() {
        return original_n;
    }
    let original_len = original_n.chars().count();
    let corrected_len = corrected_n.chars().count();
    if original_len > 0 {
        let grow_limit = ((original_len as f32) * 1.35).ceil() as usize;
        if corrected_len > grow_limit && corrected_len > original_len + 8 {
            return original_n;
        }
    }
    corrected_n
}

fn hex_to_ass(hex: &str, alpha: u8) -> String {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return format!("&H{alpha:02X}FFFFFF");
    }
    let r = &h[0..2];
    let g = &h[2..4];
    let b = &h[4..6];
    format!("&H{alpha:02X}{}{}{}", b, g, r)
}

fn default_rounded_radius() -> u32 {
    16
}

fn default_box_padding() -> u32 {
    18
}

fn default_bg_opacity() -> u8 {
    68
}

fn default_x_padding_scale() -> f32 {
    1.0
}

fn opacity_percent_to_ass_alpha(opacity: u8) -> u8 {
    let v = opacity.min(100) as f32 / 100.0;
    let alpha = 255.0 * (1.0 - v);
    alpha.round() as u8
}

fn build_rounded_ass_script(track: &SubtitleTrack, style: &SubtitleStyle, input_video: &str) -> String {
    let (play_res_x, play_res_y) = probe_video_size(input_video).unwrap_or((1920_i32, 1080_i32));
    let scale = (play_res_y as f32 / 460.0).clamp(1.2, 4.5);
    let font_size = ((style.font_size.max(12) as f32) * scale).round() as i32;
    let anchor = "\\an5";
    let align = 5_i32;

    let text_color = hex_to_ass_bgr(&style.text_color);
    let box_color = hex_to_ass_bgr(&style.background_color);
    let box_alpha = ass_alpha(opacity_percent_to_ass_alpha(style.bg_opacity));
    let padding = style.box_padding.max(4) as i32;
    let x_padding_scale = style.x_padding_scale.clamp(0.5, 1.0);
    let horizontal_padding = ((padding as f32) * x_padding_scale).round() as i32;
    let box_h = (font_size as f32 * 1.45) as i32 + padding * 2;
    let radius = (style.rounded_radius.max(2) as i32).min((box_h / 2).max(2));
    // Optical baseline compensation for CJK heavy subtitles with bold style.
    // Keeps top/bottom visual padding closer to symmetric.
    let text_optical_offset = ((font_size as f32) * 0.08).round() as i32;
    let margin = 36_i32;
    let y = match style.position.as_str() {
        "top" => margin + box_h / 2,
        "center" => play_res_y / 2,
        _ => play_res_y - margin - box_h / 2,
    };

    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {play_res_x}\nPlayResY: {play_res_y}\n"));
    out.push_str("WrapStyle: 2\nScaledBorderAndShadow: yes\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\n");
    out.push_str(&format!(
        "Style: RoundedText,PingFang SC,{font_size},&H00FFFFFF,&H000000FF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,1,0,0,{align},20,20,24,1\n"
    ));
    out.push_str("Style: RoundedShape,Arial,20,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,5,20,20,24,1\n\n");
    out.push_str("[Events]\n");
    out.push_str("Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n");

    for seg in &track.segments {
        let start = to_ass_time(seg.start);
        let end = to_ass_time(seg.end);
        let text = escape_ass_text(&seg.text);
        // 宽度估算：中文接近 1em，ASCII 接近 0.58em，空格单独减重
        let text_width: f32 = text.chars().map(|c| {
            if c.is_whitespace() {
                font_size as f32 * 0.33
            } else if c.is_ascii() {
                font_size as f32 * 0.58
            } else {
                font_size as f32 * 0.98
            }
        }).sum();
        let min_width = font_size as f32 * 6.0; // 至少 6 个汉字宽度，避免短句背景过窄
        let box_w = (text_width.max(min_width) + (horizontal_padding.max(2) * 2) as f32)
            .clamp(200.0, (play_res_x - 120) as f32) as i32;
        let shape = rounded_box_path(box_w, box_h, radius);
        let shape_x = play_res_x / 2;
        let shape_y = y;
        let shape_tag = format!(
            "{{{}\\pos({},{})\\p1\\c{}\\1a{}\\bord0\\shad0}}{}{{\\p0}}",
            anchor,
            shape_x,
            shape_y,
            box_color,
            box_alpha,
            shape
        );
        let text_tag = format!(
            "{{{}\\pos({},{})\\bord0\\shad0\\1c{}\\1a&H00&}}{}",
            anchor,
            play_res_x / 2,
            y - text_optical_offset,
            text_color,
            text
        );
        out.push_str(&format!(
            "Dialogue: 0,{start},{end},RoundedShape,,0,0,0,,{shape_tag}\n"
        ));
        out.push_str(&format!(
            "Dialogue: 1,{start},{end},RoundedText,,0,0,0,,{text_tag}\n"
        ));
    }
    out
}

fn probe_video_size(input_video: &str) -> Option<(i32, i32)> {
    let ffprobe = first_existing(&[
        "/opt/homebrew/bin/ffprobe",
        "/usr/local/bin/ffprobe",
        "/usr/bin/ffprobe",
    ])?;
    let out = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=s=x:p=0")
        .arg(input_video)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut it = s.split('x');
    let w = it.next()?.trim().parse::<i32>().ok()?;
    let h = it.next()?.trim().parse::<i32>().ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

fn probe_video_duration(input_video: &str) -> Option<f64> {
    let ffprobe = first_existing(&[
        "/opt/homebrew/bin/ffprobe",
        "/usr/local/bin/ffprobe",
        "/usr/bin/ffprobe",
    ])?;
    let out = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(input_video)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok()
}

fn escape_filter_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace(',', "\\,")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('\'', "\\'")
}

fn rounded_box_path(width: i32, height: i32, radius: i32) -> String {
    let r = radius.min((width / 2).saturating_sub(1)).min((height / 2).saturating_sub(1)).max(2);
    let k = ((r as f32) * 0.552_284_8_f32) as i32;
    let w = width.max(4);
    let h = height.max(4);
    let wr = w - r;
    let hr = h - r;
    let wrk = wr + k;
    let hrk = hr + k;
    let rk = r - k;
    format!(
        "m {r} 0 \
         l {wr} 0 \
         b {wrk} 0 {w} {rk} {w} {r} \
         l {w} {hr} \
         b {w} {hrk} {wrk} {h} {wr} {h} \
         l {r} {h} \
         b {rk} {h} 0 {hrk} 0 {hr} \
         l 0 {r} \
         b 0 {rk} {rk} 0 {r} 0 c"
    )
}


fn hex_to_ass_bgr(hex: &str) -> String {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return "&HFFFFFF&".to_string();
    }
    let r = &h[0..2];
    let g = &h[2..4];
    let b = &h[4..6];
    format!("&H{}{}{}&", b, g, r)
}

fn ass_alpha(alpha: u8) -> String {
    format!("&H{alpha:02X}&")
}

fn to_ass_time(sec: f32) -> String {
    let total_cs = (sec.max(0.0) * 100.0).round() as i64;
    let h = total_cs / 360000;
    let m = (total_cs % 360000) / 6000;
    let s = (total_cs % 6000) / 100;
    let cs = total_cs % 100;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

fn escape_ass_text(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('{', r"\{")
        .replace('}', r"\}")
        .replace('\n', r"\N")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            use tauri::menu::{MenuBuilder, MenuItem, SubmenuBuilder};
            let handle = app.handle();
            let export_item = MenuItem::with_id(app, "menu_export", "导出", true, None::<&str>)?;
            let file_menu = SubmenuBuilder::new(handle, "文件")
                .item(&export_item)
                .build()?;
            let edit_menu = SubmenuBuilder::new(handle, "编辑")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let menu = MenuBuilder::new(handle)
                .item(&file_menu)
                .item(&edit_menu)
                .build()?;
            app.set_menu(menu)?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "menu_export" {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.emit("menu-export", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            pick_video_file,
            pick_reference_file,
            pick_subtitle_file,
            pick_export_file,
            suggest_project_path,
            get_correction_runtime_config,
            get_asr_runtime_config,
            detect_local_setup,
            install_local_whisper_base,
            cut_video,
            extract_audio,
            strip_audio,
            transcribe_audio,
            burn_subtitles,
            export_subtitled_video,
            correct_subtitles,
            correct_subtitles_batch,
            load_subtitles,
            save_subtitles,
            save_project,
            load_project
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn extract_json_payload(s: &str) -> Option<String> {
    if let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) {
        if end > start {
            return Some(s[start..=end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod local_round_burn_tests {
    use super::*;

    #[test]
    fn test_local_rounded_burn_quick_20s() {
        let input_video = "/Users/pxy/PycharmProjects/video_cut_software/scripts/quick_20s.mp4";
        let subtitles_json =
            "/Users/pxy/PycharmProjects/video_cut_software/scripts/kimi_run_script.json";
        let output_video =
            "/Users/pxy/PycharmProjects/video_cut_software/scripts/quick_20s.rounded.test.mp4";

        if !Path::new(input_video).exists() || !Path::new(subtitles_json).exists() {
            eprintln!(
                "[skip] local test file missing: input={}, subtitles={}",
                input_video, subtitles_json
            );
            return;
        }

        let track = SubtitleTrack::from_json_file(subtitles_json).expect("load subtitle json");
        let style = SubtitleStyle {
            position: "bottom".to_string(),
            font_size: 17,
            text_color: "#ffe200".to_string(),
            background_color: "#000000".to_string(),
            rounded_required: true,
            rounded_radius: 16,
            box_padding: 9,
            bg_opacity: 45,
            x_padding_scale: 1.0,
        };
        let ass_content = build_rounded_ass_script(&track, &style, input_video);
        let ass_path = std::env::temp_dir().join("video_cut_studio_burn.rounded.ass");
        fs::write(&ass_path, ass_content).expect("write ass");
        ffmpeg::burn_ass_file(input_video, &ass_path, output_video).expect("burn ass");
        assert!(Path::new(output_video).exists(), "output should exist");
        eprintln!(
            "[ok] rounded burn output={}",
            output_video
        );
    }
}
