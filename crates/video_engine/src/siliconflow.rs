use crate::subtitle::{CorrectedSegment, SubtitleTrack};
use anyhow::{Context, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct SiliconFlowClient {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl SiliconFlowClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = resolve_api_key().context(
            "SILICONFLOW_API_KEY is not set（GUI 启动通常不会继承 shell 环境变量；请在项目 .env 或 ~/.video_cut_studio/.env 中配置）",
        )?;
        let base_url = std::env::var("SILICONFLOW_BASE_URL")
            .unwrap_or_else(|_| "https://api.siliconflow.cn/v1".to_string());
        let model = std::env::var("SILICONFLOW_CORRECT_MODEL")
            .or_else(|_| std::env::var("SILICONFLOW_TEXT_MODEL"))
            .unwrap_or_else(|_| "Pro/Qwen/Qwen2.5-7B-Instruct".to_string());
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .build()
            .context("failed to build http client")?;

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
        })
    }

    pub async fn correct_subtitles(
        &self,
        track: &SubtitleTrack,
        reference_script: Option<&str>,
    ) -> anyhow::Result<Vec<CorrectedSegment>> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let reference_trimmed = trim_reference(reference_script);
        let batch_size = std::env::var("SILICONFLOW_CORRECT_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(20);
        let concurrency = std::env::var("SILICONFLOW_CORRECT_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(20);

        let total = track.segments.len();
        if total == 0 {
            return Ok(Vec::new());
        }

        eprintln!(
            "[siliconflow] dispatch correction batches: total_segments={}, batch_size={}, concurrency={}",
            total, batch_size, concurrency
        );

        let sem = Arc::new(Semaphore::new(concurrency));
        let mut handles = Vec::new();
        let total_batches = total.div_ceil(batch_size);

        for (batch_no, chunk) in track.segments.chunks(batch_size).enumerate() {
            let start = batch_no * batch_size;
            let segments = chunk.to_vec();
            let url = url.clone();
            let reference = reference_trimmed.clone();
            let sem = Arc::clone(&sem);
            let client = self.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.context("acquire semaphore failed")?;
                eprintln!(
                    "[siliconflow] batch {}/{} start: range=[{}..{}], count={}",
                    batch_no + 1,
                    total_batches,
                    start,
                    start + segments.len().saturating_sub(1),
                    segments.len()
                );
                let prompt = build_prompt_for_chunk(&segments, start, reference.as_deref());
                let mut result = Vec::<CorrectedSegment>::new();
                let mut ok = false;
                for attempt in 1..=2 {
                    match client.request_corrections(&url, prompt.clone(), segments.len()).await {
                        Ok(items) => {
                            result = items;
                            ok = true;
                            break;
                        }
                        Err(e) => {
                            eprintln!(
                                "[siliconflow] batch {}/{} attempt {} failed: {}",
                                batch_no + 1,
                                total_batches,
                                attempt,
                                e
                            );
                            tokio::time::sleep(Duration::from_millis(280)).await;
                        }
                    }
                }
                if !ok {
                    eprintln!(
                        "[siliconflow] batch {}/{} fallback to keep original text (no correction applied)",
                        batch_no + 1,
                        total_batches
                    );
                }
                eprintln!(
                    "[siliconflow] batch {}/{} done: items={}",
                    batch_no + 1,
                    total_batches,
                    result.len()
                );
                Ok::<Vec<CorrectedSegment>, anyhow::Error>(result)
            }));
        }

        let mut merged = Vec::<CorrectedSegment>::new();
        for handle in handles {
            let items = handle
                .await
                .context("siliconflow batch join failed")??;
            merged.extend(items);
        }
        merged.sort_by_key(|i| i.index);
        Ok(merged)
    }

    async fn request_corrections(
        &self,
        url: &str,
        prompt: String,
        segment_count: usize,
    ) -> anyhow::Result<Vec<CorrectedSegment>> {
        eprintln!(
            "[siliconflow] start correction: model={}, segments={}, prompt_chars={}",
            self.model,
            segment_count,
            prompt.chars().count()
        );

        let req_json_mode = ChatCompletionRequest {
            model: self.model.clone(),
            temperature: 0.1,
            max_tokens: Some(default_max_tokens()),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "你是字幕校正助手。只能修正错别字/语病，不得篡改事实，不得改时间戳，不得新增原始未出现的信息。输出必须是严格 JSON 对象，格式为 {\"items\":[{\"index\":0,\"corrected_text\":\"...\"}]}。corrected_text 必须是纯文本，禁止再嵌套 JSON、禁止包含 index 标签和时间戳前缀。".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: prompt,
                },
            ],
            response_format: Some(ResponseFormat {
                r#type: "json_object".to_string(),
            }),
        };

        let wait_flag = Arc::new(AtomicBool::new(false));
        let wait_flag_clone = Arc::clone(&wait_flag);
        let model_for_log = self.model.clone();
        let start_at = Instant::now();
        let wait_logger = tokio::spawn(async move {
            while !wait_flag_clone.load(Ordering::Relaxed) {
                let elapsed = start_at.elapsed().as_secs_f32();
                eprintln!(
                    "[siliconflow] waiting model response... model={}, elapsed={:.1}s",
                    model_for_log, elapsed
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        let resp = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req_json_mode)
            .send()
            .await
            .context("failed to call SiliconFlow chat completions")?;
        wait_flag.store(true, Ordering::Relaxed);
        let _ = wait_logger.await;
        eprintln!(
            "[siliconflow] first response arrived in {:.2}s",
            start_at.elapsed().as_secs_f32()
        );

        let content = if resp.status().is_success() {
            parse_chat_content(resp).await?
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if body.to_lowercase().contains("json mode is not supported") {
                eprintln!(
                    "[siliconflow] model does not support json mode, retry without response_format"
                );
                let req_fallback = ChatCompletionRequest {
                    model: self.model.clone(),
                    temperature: 0.1,
                    max_tokens: Some(default_max_tokens()),
                    messages: req_json_mode.messages,
                    response_format: None,
                };
                let retry = self
                    .client
                    .post(url)
                    .bearer_auth(&self.api_key)
                    .json(&req_fallback)
                    .send()
                    .await
                    .context("failed to retry SiliconFlow without json mode")?;
                if !retry.status().is_success() {
                    let retry_status = retry.status();
                    let retry_body = retry.text().await.unwrap_or_default();
                    bail!("SiliconFlow API failed: {retry_status}, body: {retry_body}");
                }
                parse_chat_content(retry).await?
            } else {
                bail!("SiliconFlow API failed: {status}, body: {body}");
            }
        };

        let result =
            parse_corrected_output(&content).context("model output is not valid corrected JSON")?;
        eprintln!(
            "[siliconflow] correction parsed ok: corrected_items={}",
            result.len()
        );

        Ok(result)
    }
}

async fn parse_chat_content(resp: reqwest::Response) -> anyhow::Result<String> {
    let parsed: ChatCompletionResponse = resp
        .json()
        .await
        .context("failed to parse SiliconFlow response")?;
    parsed
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .cloned()
        .context("missing content in SiliconFlow response")
}

fn extract_json_payload(s: &str) -> Option<String> {
    if let (Some(start), Some(end)) = (s.find("```json"), s.rfind("```")) {
        let body = &s[start + 7..end];
        let trimmed = body.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            return Some(trimmed.to_string());
        }
    }
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(s[start..=end].to_string())
}

fn parse_corrected_output(content: &str) -> anyhow::Result<Vec<CorrectedSegment>> {
    let mut candidates = Vec::<String>::new();
    candidates.push(content.to_string());
    if let Some(payload) = extract_json_payload(content) {
        candidates.push(payload);
    }

    for candidate in &candidates {
        if let Ok(parsed) = serde_json::from_str::<CorrectedOutput>(candidate)
            && !parsed.items.is_empty()
        {
            return Ok(sanitize_corrected_segments(parsed.items));
        }
        if let Ok(value) = serde_json::from_str::<Value>(candidate) {
            let normalized = normalize_from_json_value(&value, 0);
            if !normalized.is_empty() {
                return Ok(normalized);
            }
            if let Some(decoded) = value.as_str() {
                let decoded_normalized = parse_corrected_output(decoded).unwrap_or_default();
                if !decoded_normalized.is_empty() {
                    return Ok(decoded_normalized);
                }
            }
        }
    }

    let jsonl = parse_json_lines(content);
    if !jsonl.is_empty() {
        return Ok(jsonl);
    }

    let sample = content
        .chars()
        .take(480)
        .collect::<String>()
        .replace('\n', "\\n");
    bail!("unable to parse corrected JSON, sample={sample}")
}

fn normalize_from_json_value(value: &Value, depth: usize) -> Vec<CorrectedSegment> {
    if depth > 4 {
        return Vec::new();
    }

    let mut out = Vec::<CorrectedSegment>::new();
    match value {
        Value::Array(arr) => {
            for item in arr {
                if let Some(seg) = value_to_corrected_segment(item) {
                    out.push(seg);
                    continue;
                }
                out.extend(normalize_from_json_value(item, depth + 1));
            }
        }
        Value::Object(map) => {
            if let Some(seg) = value_to_corrected_segment(value) {
                out.push(seg);
            }

            for key in [
                "items",
                "data",
                "result",
                "results",
                "corrections",
                "output",
                "payload",
                "content",
            ] {
                if let Some(nested) = map.get(key) {
                    out.extend(normalize_from_json_value(nested, depth + 1));
                }
            }
        }
        Value::String(text) => {
            if text.trim_start().starts_with('{')
                || text.trim_start().starts_with('[')
                || text.contains("```")
            {
                let nested = parse_corrected_output(text).unwrap_or_default();
                out.extend(nested);
            }
        }
        _ => {}
    }

    out.sort_by_key(|i| i.index);
    out.dedup_by_key(|i| i.index);
    sanitize_corrected_segments(out)
}

fn parse_json_lines(content: &str) -> Vec<CorrectedSegment> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{') && line.ends_with('}'))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value_to_corrected_segment(&value))
        .collect()
}

fn value_to_corrected_segment(value: &Value) -> Option<CorrectedSegment> {
    let index = value
        .get("index")
        .and_then(json_usize)
        .or_else(|| value.get("idx").and_then(json_usize))?;

    let raw_text = value
        .get("corrected_text")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("text").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let corrected_text = normalize_corrected_text(raw_text, index)?;

    Some(CorrectedSegment {
        index,
        corrected_text,
    })
}

fn normalize_corrected_text(raw: &str, expected_index: usize) -> Option<String> {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return None;
    }

    if let Some(extracted) = extract_text_from_embedded_json(&text, expected_index) {
        text = extracted;
    }

    text = strip_index_prefix_and_time(text, expected_index);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return None;
    }
    Some(trimmed.to_string())
}

fn sanitize_corrected_segments(items: Vec<CorrectedSegment>) -> Vec<CorrectedSegment> {
    let mut out = Vec::<CorrectedSegment>::new();
    for item in items {
        if let Some(text) = normalize_corrected_text(&item.corrected_text, item.index) {
            out.push(CorrectedSegment {
                index: item.index,
                corrected_text: text,
            });
        }
    }
    out.sort_by_key(|i| i.index);
    out.dedup_by_key(|i| i.index);
    out
}

fn extract_text_from_embedded_json(raw: &str, expected_index: usize) -> Option<String> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.contains("```")) {
        return None;
    }

    let payload = extract_json_payload(trimmed).unwrap_or_else(|| trimmed.to_string());
    let value = serde_json::from_str::<Value>(&payload).ok()?;
    match value {
        Value::Array(arr) => {
            for item in &arr {
                if let Some(index) = item
                    .get("index")
                    .and_then(json_usize)
                    .or_else(|| item.get("idx").and_then(json_usize))
                    && index == expected_index
                    && let Some(txt) = item
                        .get("corrected_text")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.get("text").and_then(|v| v.as_str()))
                {
                    return Some(txt.trim().to_string());
                }
            }
            arr.first().and_then(|first| {
                first
                    .get("corrected_text")
                    .and_then(|v| v.as_str())
                    .or_else(|| first.get("text").and_then(|v| v.as_str()))
                    .map(|s| s.trim().to_string())
            })
        }
        Value::Object(obj) => {
            if let Some(txt) = obj
                .get("corrected_text")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("text").and_then(|v| v.as_str()))
            {
                return Some(txt.trim().to_string());
            }
            for key in ["items", "data", "result", "results", "corrections", "output"] {
                if let Some(nested) = obj.get(key) {
                    let normalized = normalize_from_json_value(nested, 1);
                    if let Some(found) = normalized.iter().find(|i| i.index == expected_index) {
                        return Some(found.corrected_text.clone());
                    }
                    if let Some(first) = normalized.first() {
                        return Some(first.corrected_text.clone());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn strip_index_prefix_and_time(text: String, expected_index: usize) -> String {
    let trimmed = text.trim();
    let prefixes = [
        format!("[{expected_index}]"),
        format!("【{expected_index}】"),
        format!("#{expected_index}"),
    ];
    let mut rest = trimmed.to_string();
    for p in prefixes {
        if rest.starts_with(&p) {
            rest = rest[p.len()..].trim_start().to_string();
            break;
        }
    }

    if rest.starts_with('(') && let Some(end) = rest.find(')') {
        let inner = &rest[1..end];
        let dots = inner.chars().filter(|c| *c == '.').count();
        if inner.contains('-')
            && dots >= 2
            && inner.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == ' ')
        {
            rest = rest[end + 1..].trim_start().to_string();
        }
    }
    rest
}

fn json_usize(value: &Value) -> Option<usize> {
    if let Some(n) = value.as_u64() {
        return usize::try_from(n).ok();
    }
    if let Some(s) = value.as_str() {
        return s.trim().parse::<usize>().ok();
    }
    None
}

fn build_prompt_for_chunk(
    segments: &[crate::subtitle::SubtitleSegment],
    start_index: usize,
    reference_script: Option<&str>,
) -> String {
    let mut p = String::new();
    p.push_str("请基于以下 ASR 字幕进行轻量纠正。\\n");
    p.push_str("规则：\\n");
    p.push_str("1) 不改 index。\\n");
    p.push_str("2) 不补充原始语音中不存在的信息。\\n");
    p.push_str("3) 仅修正明显错字和不通顺表达。\\n");
    p.push_str("4) 只返回需要修改的 index，未返回表示保持原文不变。\\n");
    p.push_str("5) 若无需修改，返回 {\\\"items\\\":[]}。\\n");
    p.push_str("6) 输出 JSON: {\\\"items\\\":[{\\\"index\\\":0,\\\"corrected_text\\\":\\\"...\\\"}]}\\n");
    p.push_str("7) corrected_text 只能是字幕纯文本，不要返回 { } [ ]、index、时间戳、注释。\\n");
    p.push_str("8) 不要使用 markdown，不要 ```json 代码块。\\n\\n");

    if let Some(script) = reference_script {
        p.push_str("参考稿件（仅参考，不强制对齐）：\\n");
        p.push_str(script);
        p.push_str("\\n\\n");
    }

    p.push_str("ASR 字幕分段：\\n");
    for (i, seg) in segments.iter().enumerate() {
        let idx = start_index + i;
        p.push_str(&format!("[{idx}] ({:.2}-{:.2}) {}\\n", seg.start, seg.end, seg.text));
    }

    p
}

fn trim_reference(reference_script: Option<&str>) -> Option<String> {
    let max_chars = std::env::var("SILICONFLOW_REFERENCE_MAX_CHARS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1500);
    reference_script.map(|s| {
        if s.chars().count() <= max_chars {
            s.to_string()
        } else {
            s.chars().take(max_chars).collect::<String>()
        }
    })
}

fn resolve_api_key() -> Option<String> {
    let _ = dotenvy::dotenv();
    load_dotenv_candidates();

    if let Ok(v) = std::env::var("SILICONFLOW_API_KEY") {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    api_key_from_shell()
}

fn load_dotenv_candidates() {
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
}

fn api_key_from_shell() -> Option<String> {
    for shell in ["/bin/zsh", "/bin/bash"] {
        let Ok(output) = Command::new(shell)
            .args(["-lic", "printenv SILICONFLOW_API_KEY"])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

fn default_max_tokens() -> u32 {
    std::env::var("SILICONFLOW_CORRECT_MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(8192)
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    r#type: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CorrectedOutput {
    items: Vec<CorrectedSegment>,
}

#[cfg(test)]
mod tests {
    use super::parse_corrected_output;

    #[test]
    fn parse_embedded_json_text_field() {
        let content = r#"{"items":[{"index":6,"corrected_text":"{\"index\":6,\"corrected_text\":\"就是客户在用户授权后拿到\"}"}]}"#;
        let parsed = parse_corrected_output(content).expect("parse failed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].index, 6);
        assert_eq!(parsed[0].corrected_text, "就是客户在用户授权后拿到");
    }

    #[test]
    fn parse_text_with_index_and_time_prefix() {
        let content =
            r#"{"items":[{"index":56,"corrected_text":"[56] (165.04-166.04) 这个句话就是了"}]}"#;
        let parsed = parse_corrected_output(content).expect("parse failed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].index, 56);
        assert_eq!(parsed[0].corrected_text, "这个句话就是了");
    }

    #[test]
    fn parse_markdown_json_block() {
        let content = r#"```json
{"items":[{"index":8,"corrected_text":"我们其实很多应用都看到了"}]}
```"#;
        let parsed = parse_corrected_output(content).expect("parse failed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].index, 8);
        assert_eq!(parsed[0].corrected_text, "我们其实很多应用都看到了");
    }
}
