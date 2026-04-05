use crate::subtitle::{SubtitleSegment, SubtitleTrack};
use anyhow::{Context, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn transcribe_with_whisper_cpp(
    whisper_bin: impl AsRef<Path>,
    model_path: impl AsRef<Path>,
    wav_path: impl AsRef<Path>,
    output_json_path: impl AsRef<Path>,
    language: &str,
) -> anyhow::Result<()> {
    let out = output_json_path.as_ref();
    let stem = out
        .file_stem()
        .and_then(|s| s.to_str())
        .context("invalid output json file name")?;
    let out_dir = out
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let status = Command::new(whisper_bin.as_ref())
        .arg("-m")
        .arg(model_path.as_ref())
        .arg("-f")
        .arg(wav_path.as_ref())
        .arg("-l")
        .arg(language)
        .arg("-oj")
        .arg("-of")
        .arg(out_dir.join(stem))
        .status()
        .context("failed to execute whisper.cpp")?;

    if !status.success() {
        bail!("whisper.cpp failed with status: {status}");
    }

    Ok(())
}

pub fn load_whisper_json_to_track(
    path: impl AsRef<Path>,
    language: &str,
) -> anyhow::Result<SubtitleTrack> {
    let path_ref = path.as_ref();
    let content = std::fs::read_to_string(path_ref)
        .with_context(|| format!("failed to read whisper json: {}", path_ref.display()))?;
    let v: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse whisper json: {}", path_ref.display()))?;

    let segments = parse_segments(&v)?;
    Ok(SubtitleTrack {
        language: language.to_string(),
        segments,
    })
}

fn parse_segments(v: &Value) -> anyhow::Result<Vec<SubtitleSegment>> {
    let arr = v
        .get("transcription")
        .and_then(|x| x.get("segments"))
        .and_then(Value::as_array)
        .or_else(|| v.get("segments").and_then(Value::as_array))
        .or_else(|| v.get("transcription").and_then(Value::as_array))
        .context("no segments found in whisper json")?;

    let mut out = Vec::with_capacity(arr.len());
    for s in arr {
        let text = s
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        let (start, end) = if let (Some(st), Some(ed)) = (
            s.get("start").and_then(Value::as_f64),
            s.get("end").and_then(Value::as_f64),
        ) {
            (st as f32, ed as f32)
        } else if let (Some(t0), Some(t1)) = (
            s.get("t0").and_then(Value::as_i64),
            s.get("t1").and_then(Value::as_i64),
        ) {
            (t0 as f32 / 1000.0, t1 as f32 / 1000.0)
        } else if let (Some(of0), Some(of1)) = (
            s.get("offsets")
                .and_then(|o| o.get("from"))
                .and_then(Value::as_i64),
            s.get("offsets")
                .and_then(|o| o.get("to"))
                .and_then(Value::as_i64),
        ) {
            (of0 as f32 / 1000.0, of1 as f32 / 1000.0)
        } else {
            (0.0_f32, 0.0_f32)
        };

        out.push(SubtitleSegment { start, end, text });
    }

    Ok(out)
}
