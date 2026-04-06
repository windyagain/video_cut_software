use anyhow::{Context, bail};
use std::path::Path;
use std::process::Command;

fn resolve_ffmpeg_bin() -> String {
    if let Ok(custom) = std::env::var("FFMPEG_BIN") {
        if Path::new(&custom).exists() {
            return custom;
        }
    }

    let candidates = [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ];

    for c in candidates {
        if Path::new(c).exists() {
            return c.to_string();
        }
    }

    "ffmpeg".to_string()
}

pub fn cut_video(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    start_seconds: f32,
    duration_seconds: f32,
) -> anyhow::Result<()> {
    if duration_seconds <= 0.0 {
        bail!("duration must be > 0");
    }

    let ffmpeg_bin = resolve_ffmpeg_bin();
    let output_exec = Command::new(&ffmpeg_bin)
        .arg("-y")
        .arg("-ss")
        .arg(format!("{start_seconds:.3}"))
        .arg("-i")
        .arg(input.as_ref())
        .arg("-t")
        .arg(format!("{duration_seconds:.3}"))
        .arg("-c")
        .arg("copy")
        .arg(output.as_ref())
        .output()
        .context("failed to execute ffmpeg for video cut")?;

    if !output_exec.status.success() {
        let err = String::from_utf8_lossy(&output_exec.stderr);
        bail!(
            "ffmpeg cut failed. bin: {}, status: {}, stderr: {}",
            ffmpeg_bin,
            output_exec.status,
            err.trim()
        );
    }

    Ok(())
}

pub fn extract_audio_wav_mono16k(input: impl AsRef<Path>, output: impl AsRef<Path>) -> anyhow::Result<()> {
    let ffmpeg_bin = resolve_ffmpeg_bin();
    let output_exec = Command::new(&ffmpeg_bin)
        .arg("-y")
        .arg("-i")
        .arg(input.as_ref())
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(output.as_ref())
        .output()
        .context("failed to execute ffmpeg for audio extraction")?;

    if !output_exec.status.success() {
        let err = String::from_utf8_lossy(&output_exec.stderr);
        bail!(
            "ffmpeg extract failed. bin: {}, status: {}, stderr: {}",
            ffmpeg_bin,
            output_exec.status,
            err.trim()
        );
    }

    Ok(())
}

pub fn burn_subtitles(
    input_video: impl AsRef<Path>,
    srt_path: impl AsRef<Path>,
    output_video: impl AsRef<Path>,
    force_style: Option<&str>,
) -> anyhow::Result<()> {
    burn_subtitles_with_options(input_video, srt_path, output_video, force_style, None, None)
}

pub fn burn_subtitles_with_options(
    input_video: impl AsRef<Path>,
    srt_path: impl AsRef<Path>,
    output_video: impl AsRef<Path>,
    force_style: Option<&str>,
    resolution: Option<(u32, u32)>,
    video_bitrate: Option<&str>,
) -> anyhow::Result<()> {
    let ffmpeg_bin = resolve_ffmpeg_bin();
    let sub_path = escape_filter_path(srt_path.as_ref());

    let mut vf = if let Some(style) = force_style {
        if style.trim().is_empty() {
            format!("subtitles='{}'", sub_path)
        } else {
            let escaped_style = style.replace('\'', "\\'");
            format!("subtitles='{}':force_style='{}'", sub_path, escaped_style)
        }
    } else {
        format!("subtitles='{}'", sub_path)
    };
    if let Some((w, h)) = resolution {
        vf.push_str(&format!(",scale={w}:{h}"));
    }

    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.arg("-y")
        .arg("-i")
        .arg(input_video.as_ref())
        .arg("-vf")
        .arg(vf)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k");
    if let Some(br) = video_bitrate.filter(|s| !s.trim().is_empty()) {
        cmd.arg("-b:v").arg(br.trim());
    }
    let output_exec = cmd
        .arg(output_video.as_ref())
        .output()
        .context("failed to execute ffmpeg for subtitle burn")?;

    if !output_exec.status.success() {
        let err = String::from_utf8_lossy(&output_exec.stderr);
        bail!(
            "ffmpeg burn subtitle failed. bin: {}, status: {}, stderr: {}",
            ffmpeg_bin,
            output_exec.status,
            err.trim()
        );
    }

    Ok(())
}

pub fn burn_ass_file(
    input_video: impl AsRef<Path>,
    ass_path: impl AsRef<Path>,
    output_video: impl AsRef<Path>,
) -> anyhow::Result<()> {
    burn_ass_file_with_options(input_video, ass_path, output_video, None, None)
}

pub fn burn_ass_file_with_options(
    input_video: impl AsRef<Path>,
    ass_path: impl AsRef<Path>,
    output_video: impl AsRef<Path>,
    resolution: Option<(u32, u32)>,
    video_bitrate: Option<&str>,
) -> anyhow::Result<()> {
    let ffmpeg_bin = resolve_ffmpeg_bin();
    let sub_path = escape_filter_path(ass_path.as_ref());
    let mut vf = format!("subtitles='{}'", sub_path);
    if let Some((w, h)) = resolution {
        vf.push_str(&format!(",scale={w}:{h}"));
    }

    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.arg("-y")
        .arg("-i")
        .arg(input_video.as_ref())
        .arg("-vf")
        .arg(vf)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k");
    if let Some(br) = video_bitrate.filter(|s| !s.trim().is_empty()) {
        cmd.arg("-b:v").arg(br.trim());
    }
    let output_exec = cmd
        .arg(output_video.as_ref())
        .output()
        .context("failed to execute ffmpeg for ass burn")?;

    if !output_exec.status.success() {
        let err = String::from_utf8_lossy(&output_exec.stderr);
        bail!(
            "ffmpeg burn ass failed. bin: {}, status: {}, stderr: {}",
            ffmpeg_bin,
            output_exec.status,
            err.trim()
        );
    }

    Ok(())
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
