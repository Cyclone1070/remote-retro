use anyhow::Result;
use futures_util::StreamExt;
use std::time::Instant;
use tokio_tungstenite::connect_async;

#[tokio::main]
async fn main() -> Result<()> {
    let ws_url = "ws://100.73.151.90:48500/ws";
    println!("Connecting to live GBA Stream at {} to sample audio...", ws_url);
    let (ws_stream, _) = connect_async(ws_url).await?;
    let (_, mut read) = ws_stream.split();

    let mut captured_frames = 0;
    let mut total_audio_bytes = 0;
    let mut non_zero_samples = 0;
    let mut all_samples: Vec<i16> = Vec::new();

    let start = Instant::now();
    while captured_frames < 120 && start.elapsed().as_secs() < 5 {
        if let Some(Ok(msg)) = read.next().await {
            if msg.is_binary() {
                let bytes = msg.into_data();
                if bytes.len() >= 35 {
                    captured_frames += 1;
                    let audio_len = u16::from_le_bytes(bytes[33..35].try_into().unwrap()) as usize;
                    if audio_len > 0 && bytes.len() >= 35 + audio_len {
                        let audio_payload = &bytes[35..35 + audio_len];
                        total_audio_bytes += audio_len;
                        if let Ok(decomp) = lz4_flex::decompress_size_prepended(audio_payload) {
                            let samples = unsafe {
                                std::slice::from_raw_parts(decomp.as_ptr() as *const i16, decomp.len() / 2)
                            };
                            for &s in samples {
                                if s != 0 {
                                    non_zero_samples += 1;
                                }
                                all_samples.push(s);
                            }
                        }
                    }
                }
            }
        }
    }

    let min_sample = all_samples.iter().cloned().min().unwrap_or(0);
    let max_sample = all_samples.iter().cloned().max().unwrap_or(0);
    let rms: f64 = if !all_samples.is_empty() {
        (all_samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / all_samples.len() as f64).sqrt()
    } else {
        0.0
    };

    println!("===================================================================");
    println!(" 🎵 LIVE AUDIO STREAM PROOF (120 FRAMES SAMPLED)");
    println!("===================================================================");
    println!(" Captured Frames:          {}", captured_frames);
    println!(" Total Audio Samples:      {} stereo samples", all_samples.len());
    println!(" Non-Zero Active Samples:  {} ({:.2}%)", non_zero_samples, (non_zero_samples as f64 / all_samples.len().max(1) as f64) * 100.0);
    println!(" Peak Amplitude Range:     [{} to {}] (16-bit PCM)", min_sample, max_sample);
    println!(" Root Mean Square (RMS):   {:.2} (Audible Energy)", rms);
    println!(" Compressed Bandwidth:     {:.2} KB ({:.2} kbps @ 60 FPS)", total_audio_bytes as f64 / 1024.0, (total_audio_bytes as f64 * 8.0 * 60.0) / (captured_frames as f64 * 1000.0));
    println!("===================================================================");

    Ok(())
}
