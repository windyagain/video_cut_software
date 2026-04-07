use crate::subtitle::{SubtitleSegment, SubtitleTrack};
use anyhow::{Context, bail};
use reqwest::multipart;
use reqwest::Client;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

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

pub async fn transcribe_with_tool_asr(
    api_origin: &str,
    wav_path: impl AsRef<Path>,
    dashscope_api_key: &str,
    model: &str,
) -> anyhow::Result<(String, SubtitleTrack)> {
    let origin = api_origin.trim().trim_end_matches('/');
    if origin.is_empty() {
        bail!("api origin is empty");
    }
    if dashscope_api_key.trim().is_empty() {
        bail!("dashscope api key is empty");
    }
    let wav = wav_path.as_ref();
    let file_name = wav
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio.wav")
        .to_string();
    let file_bytes = tokio::fs::read(wav)
        .await
        .with_context(|| format!("failed to read wav file: {}", wav.display()))?;
    let part = multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("audio/wav")
        .context("failed to set mime for audio file")?;
    let form = multipart::Form::new()
        .part("file", part)
        .text("dashscope_api_key", dashscope_api_key.trim().to_string())
        .text("model", model.trim().to_string());
    let url = format!("{origin}/tool/audio_asr");
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(240))
        .build()
        .context("failed to build http client")?;
    let resp = client
        .post(url)
        .multipart(form)
        .send()
        .await
        .context("failed to call tool audio_asr api")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("audio_asr failed: status={status}, body={body}");
    }
    let track: SubtitleTrack = serde_json::from_str(&body)
        .context("failed to parse audio_asr response as subtitle track")?;
    Ok((body, track))
}

pub async fn transcribe_with_dashscope_direct(
    wav_path: impl AsRef<Path>,
    dashscope_api_key: &str,
    model: &str,
) -> anyhow::Result<(String, SubtitleTrack)> {
    if dashscope_api_key.trim().is_empty() {
        bail!("dashscope api key is empty");
    }
    let wav = wav_path.as_ref();
    if !wav.exists() {
        bail!("wav file does not exist: {}", wav.display());
    }
    let file_name = wav
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio.wav")
        .to_string();
    let file_bytes = tokio::fs::read(wav)
        .await
        .with_context(|| format!("failed to read wav file: {}", wav.display()))?;

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()
        .context("failed to build http client")?;

    // Step 1: upload wav to litterbox for public URL (fallback-safe chain).
    let upload_form = multipart::Form::new()
        .text("reqtype", "fileupload")
        .text("time", "72h")
        .part(
            "fileToUpload",
            multipart::Part::bytes(file_bytes)
                .file_name(file_name)
                .mime_str("audio/wav")
                .context("failed to set mime for upload file")?,
        );
    let upload_resp = client
        .post("https://litterbox.catbox.moe/resources/internals/api.php")
        .multipart(upload_form)
        .send()
        .await
        .context("failed to upload audio to litterbox")?;
    if !upload_resp.status().is_success() {
        let status = upload_resp.status();
        let body = upload_resp.text().await.unwrap_or_default();
        bail!("litterbox upload failed: status={status}, body={body}");
    }
    let file_url = upload_resp
        .text()
        .await
        .context("failed to read litterbox response")?
        .trim()
        .to_string();
    if !file_url.starts_with("http") {
        bail!("invalid litterbox url: {file_url}");
    }

    // Step 2: submit DashScope async transcription task.
    let submit_body = serde_json::json!({
        "model": model.trim(),
        "input": { "file_urls": [file_url] },
        "parameters": { "language_hints": ["zh"] }
    });
    let submit_resp = client
        .post("https://dashscope.aliyuncs.com/api/v1/services/audio/asr/transcription")
        .header("Authorization", format!("Bearer {}", dashscope_api_key.trim()))
        .header("X-DashScope-Async", "enable")
        .json(&submit_body)
        .send()
        .await
        .context("failed to submit dashscope asr task")?;
    let submit_status = submit_resp.status();
    let submit_text = submit_resp.text().await.unwrap_or_default();
    if !submit_status.is_success() {
        bail!("dashscope submit failed: status={submit_status}, body={submit_text}");
    }
    let submit_json: Value = serde_json::from_str(&submit_text)
        .context("failed to parse dashscope submit response")?;
    let task_id = submit_json
        .get("output")
        .and_then(|o| o.get("task_id"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .context("dashscope submit response missing output.task_id")?;

    // Step 3: poll task status.
    let mut last_output: Option<Value> = None;
    for _ in 0..120 {
        let url = format!("https://dashscope.aliyuncs.com/api/v1/tasks/{task_id}");
        let task_resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", dashscope_api_key.trim()))
            .send()
            .await
            .context("failed to poll dashscope task")?;
        let task_status = task_resp.status();
        let task_text = task_resp.text().await.unwrap_or_default();
        if !task_status.is_success() {
            bail!("dashscope task query failed: status={task_status}, body={task_text}");
        }
        let task_json: Value =
            serde_json::from_str(&task_text).context("failed to parse dashscope task response")?;
        let output = task_json.get("output").cloned().unwrap_or(Value::Null);
        last_output = Some(output.clone());
        let status = output
            .get("task_status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        if status == "SUCCEEDED" {
            break;
        }
        if status == "FAILED" {
            bail!("dashscope task failed: {task_text}");
        }
        sleep(Duration::from_secs(3)).await;
    }

    let output = last_output.context("dashscope task polling has no output")?;
    let transcription_url = output
        .get("results")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("transcription_url"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .context("dashscope task succeeded but transcription_url is missing")?;

    // Step 4: download raw transcription JSON and normalize to SubtitleTrack.
    let raw_resp = client
        .get(&transcription_url)
        .send()
        .await
        .context("failed to fetch transcription_url")?;
    let raw_status = raw_resp.status();
    let raw_body = raw_resp.text().await.unwrap_or_default();
    if !raw_status.is_success() {
        bail!("transcription_url fetch failed: status={raw_status}, body={raw_body}");
    }

    let raw_json: Value =
        serde_json::from_str(&raw_body).context("failed to parse transcription raw json")?;
    let track = parse_dashscope_raw_to_track(&raw_json, "zh");
    Ok((raw_body, track))
}

fn parse_dashscope_raw_to_track(v: &Value, language: &str) -> SubtitleTrack {
    let mut segments: Vec<SubtitleSegment> = Vec::new();
    if let Some(transcripts) = v.get("transcripts").and_then(Value::as_array) {
        for tr in transcripts {
            if let Some(sentences) = tr.get("sentences").and_then(Value::as_array) {
                for s in sentences {
                    let text = s
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let begin_ms = s
                        .get("begin_time")
                        .and_then(Value::as_f64)
                        .or_else(|| s.get("start_time").and_then(Value::as_f64))
                        .unwrap_or(0.0);
                    let end_ms = s
                        .get("end_time")
                        .and_then(Value::as_f64)
                        .or_else(|| s.get("stop_time").and_then(Value::as_f64))
                        .unwrap_or(begin_ms);
                    let start = (begin_ms / 1000.0) as f32;
                    let end = ((end_ms / 1000.0) as f32).max(start);
                    segments.push(SubtitleSegment { start, end, text });
                }
            }
        }
    }
    SubtitleTrack {
        language: language.to_string(),
        segments,
    }
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
