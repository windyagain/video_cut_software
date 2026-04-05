use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectConfig {
    input_video: String,
    clipped_video: String,
    #[serde(default)]
    rendered_video: String,
    audio_wav: String,
    whisper_bin: String,
    whisper_model: String,
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

#[derive(Debug, Clone, Deserialize)]
struct BatchCorrectedItem {
    index: usize,
    corrected_text: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BatchCorrectedOutput {
    items: Vec<BatchCorrectedItem>,
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

fn default_download_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Downloads"))
        .unwrap_or_else(|_| PathBuf::from("."))
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
async fn transcribe_audio(
    whisper_bin: String,
    model: String,
    wav: String,
    whisper_json: String,
    language: String,
    subtitles_out: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        asr::transcribe_with_whisper_cpp(&whisper_bin, &model, &wav, &whisper_json, &language)
            .map_err(|e| e.to_string())?;
        let track =
            asr::load_whisper_json_to_track(&whisper_json, &language).map_err(|e| e.to_string())?;
        track.to_json_file(&subtitles_out).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("transcribe_audio task join failed: {e}"))?
}

#[tauri::command]
async fn burn_subtitles(
    input_video: String,
    subtitles_json: String,
    output_video: String,
    style: SubtitleStyle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let track = SubtitleTrack::from_json_file(&subtitles_json).map_err(|e| e.to_string())?;
        if style.rounded_required {
            let ass_path = std::env::temp_dir().join("video_cut_studio_burn.rounded.ass");
            let ass_content = build_rounded_ass_script(&track, &style);
            fs::write(&ass_path, ass_content).map_err(|e| format!("写入ASS文件失败: {e}"))?;
            ffmpeg::burn_ass_file(&input_video, &ass_path, &output_video).map_err(|e| e.to_string())
        } else {
            let srt_path = std::env::temp_dir().join("video_cut_studio_burn.srt");
            track.to_srt_file(&srt_path).map_err(|e| e.to_string())?;
            let force_style = build_ass_style(&style);
            ffmpeg::burn_subtitles(&input_video, &srt_path, &output_video, Some(&force_style))
                .map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| format!("burn_subtitles task join failed: {e}"))?
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
            seg.text = item.corrected_text;
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
            if seg.text != item.corrected_text {
                applied += 1;
            }
            seg.text = item.corrected_text;
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

fn opacity_percent_to_ass_alpha(opacity: u8) -> u8 {
    let v = opacity.min(100) as f32 / 100.0;
    let alpha = 255.0 * (1.0 - v);
    alpha.round() as u8
}

fn build_rounded_ass_script(track: &SubtitleTrack, style: &SubtitleStyle) -> String {
    let play_res_x = 1920_i32;
    let play_res_y = 1080_i32;
    let font_size = style.font_size.max(20) as i32;
    let (anchor, y, align) = match style.position.as_str() {
        "top" => ("\\an8", 120_i32, 8_i32),
        "center" => ("\\an5", play_res_y / 2, 5_i32),
        _ => ("\\an2", play_res_y - 120, 2_i32),
    };

    let text_color = hex_to_ass(&style.text_color, 0x00);
    let box_color = hex_to_ass(
        &style.background_color,
        opacity_percent_to_ass_alpha(style.bg_opacity),
    );
    let padding = style.box_padding.max(4) as i32;
    let box_h = (font_size as f32 * 1.45) as i32 + padding * 2;
    let radius = (style.rounded_radius.max(2) as i32).min((box_h / 2).max(2));

    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {play_res_x}\nPlayResY: {play_res_y}\n"));
    out.push_str("WrapStyle: 2\nScaledBorderAndShadow: yes\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\n");
    out.push_str(&format!(
        "Style: RoundedText,PingFang SC,{font_size},{text_color},&H000000FF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,1,0,0,{align},20,20,24,1\n"
    ));
    out.push_str("Style: RoundedShape,Arial,20,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,2,20,20,24,1\n\n");
    out.push_str("[Events]\n");
    out.push_str("Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n");

    for seg in &track.segments {
        let start = to_ass_time(seg.start);
        let end = to_ass_time(seg.end);
        let text = escape_ass_text(&seg.text);
        let char_count = text.chars().count().max(4) as i32;
        let box_w = ((char_count as f32 * font_size as f32 * 0.62) as i32 + padding * 2)
            .clamp(260, play_res_x - 120);
        let shape = rounded_box_path(box_w, box_h, radius);
        let shape_tag = format!(
            "{{{}\\pos({},{})\\p1\\1c{}\\bord0\\shad0}}{}",
            anchor,
            play_res_x / 2,
            y,
            box_color,
            shape
        );
        let text_tag = format!(
            "{{{}\\pos({},{})\\bord0\\shad0\\1c{}}}{}",
            anchor,
            play_res_x / 2,
            y,
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

fn rounded_box_path(width: i32, height: i32, radius: i32) -> String {
    let w2 = width / 2;
    let h2 = height / 2;
    let r = radius.min(w2 - 1).min(h2 - 1).max(2);
    let k = ((r as f32) * 0.552_284_8_f32) as i32;
    // Centered at 0,0. ASS vector with cubic bezier.
    format!(
        "m {} {} l {} {} b {} {} {} {} {} {} l {} {} b {} {} {} {} {} {} l {} {} b {} {} {} {} {} {} l {} {} b {} {} {} {} {} {}",
        -w2 + r, -h2,
        w2 - r, -h2,
        w2 - r + k, -h2, w2, -h2 + r - k, w2, -h2 + r,
        w2, h2 - r,
        w2, h2 - r + k, w2 - r + k, h2, w2 - r, h2,
        -w2 + r, h2,
        -w2 + r - k, h2, -w2, h2 - r + k, -w2, h2 - r,
        -w2, -h2 + r,
        -w2, -h2 + r - k, -w2 + r - k, -h2, -w2 + r, -h2
    )
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
        .invoke_handler(tauri::generate_handler![
            pick_video_file,
            pick_reference_file,
            pick_subtitle_file,
            suggest_project_path,
            get_correction_runtime_config,
            detect_local_setup,
            install_local_whisper_base,
            cut_video,
            extract_audio,
            transcribe_audio,
            burn_subtitles,
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
