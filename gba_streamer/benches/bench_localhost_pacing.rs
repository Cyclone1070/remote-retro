use anyhow::Result;
use std::time::{Duration, Instant};

const TOTAL_SAMPLES: usize = 1000;
const WARMUP_SAMPLES: usize = 60;

fn main() -> Result<()> {
    let core_path = std::env::var("GBA_CORE").unwrap_or_else(|_| "/usr/lib64/libretro/mgba_libretro.so".to_string());
    let rom_path = std::env::var("GBA_ROM").unwrap_or_else(|_| "/tmp/test_rom.gba".to_string());

    println!("===================================================================");
    println!(" 🏆 LOCAL HOST PLAY PACING BASELINE BENCHMARK (NO NETWORK)");
    println!("===================================================================");
    println!(" Core: {}", core_path);
    println!(" ROM:  {}", rom_path);

    let mut core = gba_streamer::core::RetroCore::load(&core_path, &rom_path)?;
    core.set_runahead_frames(0);

    let frame_budget = Duration::from_micros(16_667);
    let mut next_frame_time = Instant::now();
    let mut intervals_ms = Vec::with_capacity(TOTAL_SAMPLES);
    let mut last_t = Instant::now();

    for i in 0..TOTAL_SAMPLES {
        next_frame_time += frame_budget;

        // Step emulator directly
        let (_sim_us, _frame, _audio) = core.step();

        let now = Instant::now();
        if now < next_frame_time {
            let remaining = next_frame_time - now;
            if remaining > Duration::from_millis(2) {
                std::thread::sleep(remaining - Duration::from_millis(2));
            }
            while Instant::now() < next_frame_time {
                std::hint::spin_loop();
            }
        } else if now - next_frame_time > frame_budget * 2 {
            next_frame_time = now;
        }

        let finish_t = Instant::now();
        let dt = finish_t.duration_since(last_t).as_secs_f64() * 1000.0;
        last_t = finish_t;

        if i >= WARMUP_SAMPLES {
            intervals_ms.push(dt);
        }
    }

    let n = intervals_ms.len() as f64;
    let mean_dt: f64 = intervals_ms.iter().sum::<f64>() / n;
    let variance: f64 = intervals_ms.iter().map(|&x| (x - mean_dt).powi(2)).sum::<f64>() / (n - 1.0);
    let jitter_sigma = variance.sqrt();

    let mut sorted = intervals_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50 = sorted[(n * 0.50) as usize];
    let p95 = sorted[(n * 0.95) as usize];
    let p99 = sorted[(n * 0.99) as usize];
    let min_dt = sorted[0];
    let max_dt = sorted[sorted.len() - 1];

    let macro_stutters = intervals_ms.iter().filter(|&&dt| dt >= 33.33).count();
    let micro_stutters = intervals_ms.iter().filter(|&&dt| (dt > 20.0 || dt < 13.3) && dt < 33.33).count();
    let p1_idx = (n * 0.01) as usize;
    let p01_idx = (n * 0.001) as usize;
    let p1_delta = sorted[sorted.len() - 1 - p1_idx];
    let p01_delta = sorted[sorted.len() - 1 - p01_idx];

    let p1_low_fps = 1000.0 / p1_delta;
    let p01_low_fps = 1000.0 / p01_delta;
    let fps = 1000.0 / mean_dt;

    println!(" Evaluated Frames:        {}", intervals_ms.len());
    println!(" Delivered FPS:           {:.2} FPS", fps);
    println!(" Mean Frame Interval:     {:.3} ms (Target: 16.667 ms)", mean_dt);
    println!(" Pacing Jitter (σ):       {:.3} ms", jitter_sigma);
    println!(" Macro-Stutters (>=33ms): {} ({:.2}%)", macro_stutters, (macro_stutters as f64 / n) * 100.0);
    println!(" Micro-Stutters (uneven): {} ({:.2}%)", micro_stutters, (micro_stutters as f64 / n) * 100.0);
    println!(" 1% Low Framerate (P1):   {:.2} FPS", p1_low_fps);
    println!(" 0.1% Low Framerate:      {:.2} FPS", p01_low_fps);
    println!(" P50 / P95 / P99:         {:.2} ms / {:.2} ms / {:.2} ms", p50, p95, p99);
    println!(" Min / Max Frame Time:    {:.2} ms / {:.2} ms", min_dt, max_dt);
    println!("===================================================================");

    Ok(())
}
