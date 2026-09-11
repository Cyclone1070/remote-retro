use anyhow::{Context, Result};
use gba_streamer::codec::{PaletteEncoder, PpuStateEncoder, TOTAL_PIXELS};
use gba_streamer::core::RetroCore;
use std::time::Instant;

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() { return 0.0; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((v.len() - 1) as f64 * p).round() as usize;
    v[idx]
}

struct BenchResults {
    ppu_avg_kb: f64,
    ppu_mbps: f64,
    ppu_us: f64,
    raw_tile_avg_kb: f64,
    raw_tile_mbps: f64,
    raw_tile_us: f64,
    xor_tile_avg_kb: f64,
    xor_tile_mbps: f64,
    xor_tile_us: f64,
}

fn run_game_bench(rom_name: &str, rom_path: &str, total_frames: usize, warmup_frames: usize, is_worms: bool) -> Result<BenchResults> {
    let core_path = "/usr/lib64/libretro/mgba_libretro.so";
    let mut core = RetroCore::load(core_path, rom_path).context("Failed to load core")?;

    let mut ppu_enc = PpuStateEncoder::new();
    let mut pal_delta_enc = PaletteEncoder::new();

    let mut vram_buf = vec![0u8; 98304];
    let mut pal_buf = vec![0u8; 1024];
    let mut oam_buf = vec![0u8; 1024];
    let mut io_buf = vec![0u8; 128];

    let mut prev_raw16 = vec![0u16; TOTAL_PIXELS];

    let mut ppu_sizes = Vec::new();
    let mut ppu_times = Vec::new();
    let mut raw_sizes = Vec::new();
    let mut raw_times = Vec::new();
    let mut xor_sizes = Vec::new();
    let mut xor_times = Vec::new();

    for frame_idx in 0..total_frames {
        let mask = if is_worms {
            let cycle = frame_idx % 40;
            if frame_idx < warmup_frames {
                if cycle < 8 { 1 << 0 } else if cycle >= 20 && cycle < 28 { 1 << 3 } else { 0 }
            } else {
                let f = frame_idx - warmup_frames;
                let mut m = 0;
                if (f / 45) % 2 == 0 { m |= 1 << 4; } else { m |= 1 << 5; }
                if f % 90 < 15 { m |= 1 << 0; }
                if f % 180 >= 120 && f % 180 < 150 { m |= 1 << 1; }
                m
            }
        } else {
            // Sushi the cat: run right, jump
            let cycle = frame_idx % 60;
            if cycle < 40 { (1 << 4) | (if cycle % 30 < 10 { 1 << 0 } else { 0 }) } else { 1 << 5 }
        };

        core.set_input(mask);
        let (_, _, _) = core.step_ppu(&mut vram_buf, &mut pal_buf, &mut oam_buf, &mut io_buf);

        if frame_idx < warmup_frames {
            continue;
        }

        let raw16 = core.get_last_frame();
        if raw16.len() != TOTAL_PIXELS {
            continue;
        }

        // 1. PPU State
        let t0 = Instant::now();
        let ppu_payload = ppu_enc.encode(&oam_buf, &io_buf, &pal_buf, &vram_buf);
        ppu_times.push(t0.elapsed().as_micros() as f64);
        ppu_sizes.push(ppu_payload.len() as f64);

        // 2. 16-bit RGB Tile Delta (Raw)
        let t1 = Instant::now();
        let (_, b_raw) = pal_delta_enc.encode(&raw16);
        raw_times.push(t1.elapsed().as_micros() as f64);
        raw_sizes.push(b_raw.len() as f64);

        // 3. 16-bit RGB Tile Delta (XOR)
        let t2 = Instant::now();
        {
            let mut changed = 0u16;
            let mut payload = Vec::with_capacity(2 + 600 * 130);
            payload.extend_from_slice(&0u16.to_le_bytes());
            for by in 0..20 {
                for bx in 0..30 {
                    let b_idx = (by * 30 + bx) as u16;
                    let mut is_ch = false;
                    for py in 0..8 {
                        let y = by * 8 + py;
                        for px in 0..8 {
                            let p = y * 240 + px;
                            if raw16[p] != prev_raw16[p] { is_ch = true; break; }
                        }
                        if is_ch { break; }
                    }
                    if is_ch {
                        changed += 1;
                        payload.extend_from_slice(&b_idx.to_le_bytes());
                        payload.push(2); // Mode 2: XOR block
                        for py in 0..8 {
                            let y = by * 8 + py;
                            for px in 0..8 {
                                let p = y * 240 + px;
                                let diff = raw16[p] ^ prev_raw16[p];
                                payload.extend_from_slice(&diff.to_le_bytes());
                            }
                        }
                    }
                }
            }
            let cb = changed.to_le_bytes();
            payload[0..2].copy_from_slice(&cb);
            let comp = lz4_flex::compress_prepend_size(&payload);
            xor_times.push(t2.elapsed().as_micros() as f64);
            xor_sizes.push(comp.len() as f64);
        }
        prev_raw16.copy_from_slice(&raw16);
    }

    let p_avg = mean(&ppu_sizes);
    let r_avg = mean(&raw_sizes);
    let x_avg = mean(&xor_sizes);

    Ok(BenchResults {
        ppu_avg_kb: p_avg / 1024.0,
        ppu_mbps: (p_avg * 8.0 * 60.0) / 1_000_000.0,
        ppu_us: mean(&ppu_times),
        raw_tile_avg_kb: r_avg / 1024.0,
        raw_tile_mbps: (r_avg * 8.0 * 60.0) / 1_000_000.0,
        raw_tile_us: mean(&raw_times),
        xor_tile_avg_kb: x_avg / 1024.0,
        xor_tile_mbps: (x_avg * 8.0 * 60.0) / 1_000_000.0,
        xor_tile_us: mean(&xor_times),
    })
}

fn main() -> Result<()> {
    println!("==========================================================================================");
    println!(" ⚔️  PPU STATE vs PIXEL TILE DELTA (RAW vs XOR) ON 2D GAMES");
    println!("==========================================================================================");

    // 1. Sushi The Cat (Pure 2D Tile/Sprite scrolling platformer)
    let res_sushi = run_game_bench("Sushi The Cat", "/tmp/test_rom.gba", 1500, 300, false)?;

    // 2. Worms World Party (2D Destructible Bitmap Terrain)
    let res_worms = run_game_bench("Worms World Party", "/tmp/worms.gba", 2500, 1500, true)?;

    println!("\nGame 1: Sushi The Cat (Pure 2D Hardware Tilemap + OAM Sprites, Mode 0)");
    println!("{:<35} | {:<10} | {:<12} | {:<10}", "Streaming Method", "Avg Frame", "Bitrate@60", "Host Encode");
    println!("----------------------------------------------------------------------------------");
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "PPU State Streaming", res_sushi.ppu_avg_kb, res_sushi.ppu_mbps, res_sushi.ppu_us);
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel Tile Delta (Raw blocks)", res_sushi.raw_tile_avg_kb, res_sushi.raw_tile_mbps, res_sushi.raw_tile_us);
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel Tile Delta (XOR blocks)", res_sushi.xor_tile_avg_kb, res_sushi.xor_tile_mbps, res_sushi.xor_tile_us);

    println!("\nGame 2: Worms World Party (2D Destructible Bitmap Terrain, Mode 4)");
    println!("{:<35} | {:<10} | {:<12} | {:<10}", "Streaming Method", "Avg Frame", "Bitrate@60", "Host Encode");
    println!("----------------------------------------------------------------------------------");
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "PPU State Streaming", res_worms.ppu_avg_kb, res_worms.ppu_mbps, res_worms.ppu_us);
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel Tile Delta (Raw blocks)", res_worms.raw_tile_avg_kb, res_worms.raw_tile_mbps, res_worms.raw_tile_us);
    println!("{:<35} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel Tile Delta (XOR blocks)", res_worms.xor_tile_avg_kb, res_worms.xor_tile_mbps, res_worms.xor_tile_us);
    println!("==========================================================================================\n");

    Ok(())
}
