use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleSegment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub segments: Vec<SubtitleSegment>,
}

impl SubtitleTrack {
    pub fn from_json_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path_ref = path.as_ref();
        let content = std::fs::read_to_string(path_ref)
            .with_context(|| format!("failed to read subtitle file: {}", path_ref.display()))?;
        let parsed = serde_json::from_str::<Self>(&content)
            .with_context(|| format!("failed to parse subtitle json: {}", path_ref.display()))?;
        Ok(parsed)
    }

    pub fn to_json_file(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path_ref = path.as_ref();
        let content = serde_json::to_string_pretty(self).context("failed to serialize subtitle track")?;
        std::fs::write(path_ref, content)
            .with_context(|| format!("failed to write subtitle file: {}", path_ref.display()))?;
        Ok(())
    }

    pub fn to_srt_string(&self) -> String {
        let mut out = String::new();
        for (idx, seg) in self.segments.iter().enumerate() {
            out.push_str(&(idx + 1).to_string());
            out.push('\n');
            out.push_str(&format!(
                "{} --> {}\n",
                sec_to_srt(seg.start),
                sec_to_srt(seg.end)
            ));
            out.push_str(seg.text.trim());
            out.push_str("\n\n");
        }
        out
    }

    pub fn to_srt_file(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path_ref = path.as_ref();
        std::fs::write(path_ref, self.to_srt_string())
            .with_context(|| format!("failed to write srt file: {}", path_ref.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectedSegment {
    pub index: usize,
    pub corrected_text: String,
}

fn sec_to_srt(sec: f32) -> String {
    let total_ms = (sec.max(0.0) * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}
