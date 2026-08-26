use anyhow::Result;
use futures_util::StreamExt;
use std::fs::File;
use std::io::Write;
use tokio_tungstenite::connect_async;

#[tokio::main]
async fn main() -> Result<()> {
    let ws_url = "ws://100.73.151.90:48500/ws";
    println!("Recording 3 seconds of live audio from {}...", ws_url);
    let (ws_stream, _) = connect_async(ws_url).await?;
    let (_, mut read) = ws_stream.split();

    let mut pcm_bytes = Vec::new();
    let mut frames = 0;

    while frames < 180 {
        if let Some(Ok(msg)) = read.next().await {
            if msg.is_binary() {
                let bytes = msg.into_data();
                if bytes.len() >= 35 {
                    frames += 1;
                    let audio_len = u16::from_le_bytes(bytes[33..35].try_into().unwrap()) as usize;
                    if audio_len > 0 && bytes.len() >= 35 + audio_len {
                        let audio_payload = &bytes[35..35 + audio_len];
                        if let Ok(decomp) = lz4_flex::decompress_size_prepended(audio_payload) {
                            pcm_bytes.extend_from_slice(&decomp);
                        }
                    }
                }
            }
        }
    }

    let mut wav_file = File::create("/tmp/gba_live_audio.wav")?;
    let num_samples = (pcm_bytes.len() / 4) as u32;
    let data_chunk_size = pcm_bytes.len() as u32;
    let total_file_size = 36 + data_chunk_size;

    wav_file.write_all(b"RIFF")?;
    wav_file.write_all(&total_file_size.to_le_bytes())?;
    wav_file.write_all(b"WAVEfmt ")?;
    wav_file.write_all(&16u32.to_le_bytes())?; // Subchunk1Size (16 for PCM)
    wav_file.write_all(&1u16.to_le_bytes())?;  // AudioFormat (1 = PCM)
    wav_file.write_all(&2u16.to_le_bytes())?;  // NumChannels (2 = Stereo)
    wav_file.write_all(&44100u32.to_le_bytes())?; // SampleRate
    wav_file.write_all(&(44100u32 * 4).to_le_bytes())?; // ByteRate (SampleRate * NumChannels * BitsPerSample/8)
    wav_file.write_all(&4u16.to_le_bytes())?;  // BlockAlign (NumChannels * BitsPerSample/8)
    wav_file.write_all(&16u16.to_le_bytes())?; // BitsPerSample
    wav_file.write_all(b"data")?;
    wav_file.write_all(&data_chunk_size.to_le_bytes())?;
    wav_file.write_all(&pcm_bytes)?;

    println!("Successfully recorded {} stereo samples to /tmp/gba_live_audio.wav ({} frames)", num_samples, frames);
    Ok(())
}
