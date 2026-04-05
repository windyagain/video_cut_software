use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use video_engine::asr;
use video_engine::ffmpeg;
use video_engine::siliconflow::SiliconFlowClient;
use video_engine::subtitle::SubtitleTrack;

const WAV_NAME: &str = "cap.wav";
const RAW_JSON_NAME: &str = "cap.raw.json";
const ASR_JSON_NAME: &str = "cap.asr.json";
const CORRECTED_JSON_NAME: &str = "cap.corrected.json";
const SRT_NAME: &str = "cap.srt";
const OUTPUT_VIDEO_NAME: &str = "cap.subtitled.mp4";
const DEFAULT_REFERENCE_DOC: &str = "/Users/pxy/PycharmProjects/oauth_login_multi_project/FLOW.md";
const DEFAULT_ASR_JSON: &str =
    "/Users/pxy/PycharmProjects/video_cut_software/crates/video_engine/tests/data/full_pipeline_asr.json";

fn env_or_panic(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("missing env var: {key}"))
}

fn workspace_tmp_dir() -> PathBuf {
    let root = std::env::var("VCS_TEST_OUTPUT_DIR").unwrap_or_else(|_| {
        "/Users/pxy/PycharmProjects/video_cut_software/crates/video_engine/tests/data/out"
            .to_string()
    });
    let p = PathBuf::from(root);
    fs::create_dir_all(&p).expect("failed to create output dir");
    p
}

fn paths() -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let out_dir = workspace_tmp_dir();
    (
        out_dir.join(WAV_NAME),
        out_dir.join(RAW_JSON_NAME),
        out_dir.join(ASR_JSON_NAME),
        out_dir.join(CORRECTED_JSON_NAME),
        out_dir.join(SRT_NAME),
        out_dir.join(OUTPUT_VIDEO_NAME),
    )
}

fn read_reference_text() -> Option<String> {
    if let Ok(p) = std::env::var("VCS_TEST_REFERENCE") {
        return fs::read_to_string(p).ok();
    }

    if Path::new(DEFAULT_REFERENCE_DOC).exists() {
        return fs::read_to_string(DEFAULT_REFERENCE_DOC).ok();
    }

    None
}

fn ensure_extract_and_transcribe() -> PathBuf {
    let input_video = env_or_panic("VCS_TEST_INPUT_VIDEO");
    let whisper_bin = env_or_panic("VCS_TEST_WHISPER_BIN");
    let whisper_model = env_or_panic("VCS_TEST_WHISPER_MODEL");
    let lang = std::env::var("VCS_TEST_LANGUAGE").unwrap_or_else(|_| "zh".to_string());

    let (wav, raw, asr_json, _, _, _) = paths();

    if asr_json.exists() {
        return asr_json;
    }

    ffmpeg::extract_audio_wav_mono16k(&input_video, &wav).expect("extract audio failed");
    asr::transcribe_with_whisper_cpp(&whisper_bin, &whisper_model, &wav, &raw, &lang)
        .expect("whisper transcribe failed");
    let track = asr::load_whisper_json_to_track(&raw, &lang).expect("parse whisper json failed");
    assert!(!track.segments.is_empty(), "asr segments should not be empty");
    track.to_json_file(&asr_json).expect("write asr json failed");

    asr_json
}

async fn ensure_corrected_subtitles() -> PathBuf {
    let _ = dotenvy::dotenv();
    let (_, _, asr_json, corrected_json, _, _) = paths();

    if corrected_json.exists() {
        return corrected_json;
    }

    let input = if let Ok(custom) = std::env::var("VCS_TEST_SUBTITLES_JSON") {
        PathBuf::from(custom)
    } else if Path::new(DEFAULT_ASR_JSON).exists() {
        PathBuf::from(DEFAULT_ASR_JSON)
    } else {
        if !asr_json.exists() {
            let _ = ensure_extract_and_transcribe();
        }
        asr_json
    };

    assert!(
        input.exists(),
        "subtitle input not found: {}",
        input.display()
    );

    let track = SubtitleTrack::from_json_file(&input).expect("read subtitle json failed");
    eprintln!(
        "[test] correction input loaded: file={}, segments={}",
        input.display(),
        track.segments.len()
    );
    let client = SiliconFlowClient::from_env().expect("init siliconflow client failed");
    let t0 = Instant::now();
    let corrected = client
        .correct_subtitles(&track, read_reference_text().as_deref())
        .await
        .expect("correct subtitles failed");
    eprintln!(
        "[test] correction done: corrected_items={}, elapsed={:.2}s",
        corrected.len(),
        t0.elapsed().as_secs_f32()
    );

    let mut new_track = track.clone();
    for item in corrected {
        if let Some(seg) = new_track.segments.get_mut(item.index) {
            seg.text = item.corrected_text;
        }
    }

    new_track
        .to_json_file(&corrected_json)
        .expect("write corrected subtitle failed");

    corrected_json
}

#[test]
#[ignore = "manual capability test; requires local video and ffmpeg/whisper"]
fn test_extract_and_transcribe_capability() {
    let asr_json = ensure_extract_and_transcribe();
    assert!(
        asr_json.exists(),
        "asr output not found: {}",
        asr_json.display()
    );
}

#[tokio::test]
#[ignore = "manual capability test; requires SILICONFLOW_API_KEY"]
async fn test_subtitle_correction_capability() {
    let corrected = ensure_corrected_subtitles().await;
    assert!(
        corrected.exists(),
        "corrected subtitle output not found: {}",
        corrected.display()
    );
}

#[tokio::test]
#[ignore = "manual capability test; requires video and subtitles (or transcribe env)"]
async fn test_burn_subtitle_capability() {
    let input_video = env_or_panic("VCS_TEST_INPUT_VIDEO");
    let (_, _, asr_json, corrected_json, srt, output_video) = paths();

    let subtitle_json = if let Ok(custom) = std::env::var("VCS_TEST_SUBTITLES_JSON") {
        PathBuf::from(custom)
    } else if Path::new(DEFAULT_ASR_JSON).exists() {
        PathBuf::from(DEFAULT_ASR_JSON)
    } else if corrected_json.exists() {
        corrected_json
    } else {
        // If correction is unavailable, fallback to ASR result.
        if !asr_json.exists() {
            let _ = ensure_extract_and_transcribe();
        }
        asr_json
    };

    assert!(
        subtitle_json.exists(),
        "subtitle input not found: {}",
        subtitle_json.display()
    );

    let track = SubtitleTrack::from_json_file(&subtitle_json).expect("read subtitle json failed");
    track.to_srt_file(&srt).expect("write srt failed");

    ffmpeg::burn_subtitles(
        &input_video,
        &srt,
        &output_video,
        Some(
            "Alignment=2,FontSize=36,PrimaryColour=&H00FFFF00,BackColour=&H00000000,BorderStyle=3,Outline=1,Shadow=0",
        ),
    )
    .expect("burn subtitles failed");

    assert!(
        output_video.exists(),
        "burned video output not found: {}",
        output_video.display()
    );
}
