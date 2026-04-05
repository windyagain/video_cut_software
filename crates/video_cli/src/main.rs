use anyhow::Context;
use clap::{Parser, Subcommand};
use video_engine::asr;
use video_engine::ffmpeg;
use video_engine::siliconflow::SiliconFlowClient;
use video_engine::subtitle::SubtitleTrack;

#[derive(Debug, Parser)]
#[command(name = "video-cli")]
#[command(about = "Local-first video processing CLI for MVP", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Cut {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        start: f32,
        #[arg(long)]
        duration: f32,
    },
    ExtractAudio {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },
    Transcribe {
        #[arg(long)]
        whisper_bin: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        wav: String,
        #[arg(long)]
        whisper_json: String,
        #[arg(long, default_value = "zh")]
        language: String,
        #[arg(long)]
        subtitles_out: String,
    },
    CorrectSubtitles {
        #[arg(long)]
        subtitles: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        reference: Option<String>,
    },
    BurnSubtitles {
        #[arg(long)]
        input_video: String,
        #[arg(long)]
        subtitles_json: String,
        #[arg(long)]
        output_video: String,
        #[arg(long, default_value = "bottom")]
        position: String,
        #[arg(long, default_value_t = 17)]
        font_size: u32,
        #[arg(long, default_value = "#ffe200")]
        text_color: String,
        #[arg(long, default_value = "#000000")]
        background_color: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Cut {
            input,
            output,
            start,
            duration,
        } => {
            ffmpeg::cut_video(input, output, start, duration)?;
            println!("cut completed");
        }
        Commands::ExtractAudio { input, output } => {
            ffmpeg::extract_audio_wav_mono16k(input, output)?;
            println!("audio extraction completed");
        }
        Commands::Transcribe {
            whisper_bin,
            model,
            wav,
            whisper_json,
            language,
            subtitles_out,
        } => {
            asr::transcribe_with_whisper_cpp(&whisper_bin, &model, &wav, &whisper_json, &language)?;
            let track = asr::load_whisper_json_to_track(&whisper_json, &language)?;
            track.to_json_file(&subtitles_out)?;
            println!("transcription completed");
        }
        Commands::CorrectSubtitles {
            subtitles,
            output,
            reference,
        } => {
            let track = SubtitleTrack::from_json_file(&subtitles)?;
            let reference_content = match reference {
                Some(path) => Some(
                    std::fs::read_to_string(path)
                        .context("failed to read reference script file")?,
                ),
                None => None,
            };

            let client = SiliconFlowClient::from_env()?;
            let corrected = client
                .correct_subtitles(&track, reference_content.as_deref())
                .await?;

            let mut new_track = track.clone();
            for item in corrected {
                if let Some(seg) = new_track.segments.get_mut(item.index) {
                    seg.text = item.corrected_text;
                }
            }

            new_track.to_json_file(output)?;
            println!("subtitle correction completed");
        }
        Commands::BurnSubtitles {
            input_video,
            subtitles_json,
            output_video,
            position,
            font_size,
            text_color,
            background_color,
        } => {
            let track = SubtitleTrack::from_json_file(&subtitles_json)?;
            let srt_path = std::env::temp_dir().join("video_cli_burn.srt");
            track.to_srt_file(&srt_path)?;
            let force_style = build_ass_style(&position, font_size, &text_color, &background_color);
            ffmpeg::burn_subtitles(&input_video, &srt_path, &output_video, Some(&force_style))?;
            println!("subtitle burn completed: {}", output_video);
        }
    }

    Ok(())
}

fn build_ass_style(position: &str, font_size: u32, text_color: &str, background_color: &str) -> String {
    let align = match position {
        "top" => 8,
        "center" => 5,
        _ => 2,
    };
    let primary = hex_to_ass(text_color, 0x00);
    let back = hex_to_ass(background_color, 0x88);
    format!(
        "Alignment={align},FontSize={font_size},PrimaryColour={primary},BackColour={back},BorderStyle=4,Bold=1,Outline=0,Shadow=0,MarginV=24,Spacing=0"
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
