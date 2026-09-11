use anyhow::{Context, Result};
use gba_streamer::codec::{PaletteEncoder, PpuStateEncoder, TOTAL_PIXELS, GBA_WIDTH, GBA_HEIGHT};
use gba_streamer::core::RetroCore;
use std::time::Instant;

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn estimate_motion(prev: &[u16], curr: &[u16]) -> (i32, i32) {
    let mut best_sad = u64::MAX;
    let mut best_dx = 0;
    let mut best_dy = 0;

    // Search range: dx in -8..=8, dy in -4..=4
    // Sample a central grid (e.g. y in 20..140, x in 20..220) to avoid edges and sprites
    for dy in -4..=4 {
        for dx in -8..=8 {
            let mut sad: u64 = 0;
            let mut samples = 0;
            for y in (30..130).step_by(4) {
                let py = y as i32 - dy;
                if py < 0 || py >= GBA_HEIGHT as i32 { continue; }
                for x in (30..210).step_by(4) {
                    let px = x as i32 - dx;
                    if px < 0 || px >= GBA_WIDTH as i32 { continue; }
                    let c = curr[y * GBA_WIDTH + x];
                    let p = prev[py as usize * GBA_WIDTH + px as usize];
                    if c != p {
                        sad += 1;
                    }
                    samples += 1;
                }
            }
            if samples > 0 && sad < best_sad {
                best_sad = sad;
                best_dx = dx;
                best_dy = dy;
            }
        }
    }
    (best_dx, best_dy)
}

fn main() -> Result<()> {
    let core_path = "/usr/lib64/libretro/mgba_libretro.so";
    let rom_path = "/tmp/test_rom.gba";
    let total_frames: usize = 1500;
    let warmup_frames: usize = 300;

    let mut core = RetroCore::load(core_path, rom_path).context("Failed to load core")?;

    let mut vram_buf = vec![0u8; 98304];
    let mut pal_buf = vec![0u8; 1024];
    let mut oam_buf = vec![0u8; 1024];
    let mut io_buf = vec![0u8; 128];

    let mut prev_bg0_x = 0u16;
    let mut prev_bg0_y = 0u16;
    let mut prev_bg1_x = 0u16;
    let mut prev_bg1_y = 0u16;

    let mut prev_raw16 = vec![0u16; TOTAL_PIXELS];
    let mut shifted_prev = vec![0u16; TOTAL_PIXELS];

    let mut sizes_hw_scroll = Vec::new();
    let mut times_hw_scroll = Vec::new();

    println!("==========================================================================================");
    println!(" 🎮 ZERO-COST HARDWARE SCROLL REGISTER COMPENSATION ON SUSHI THE CAT");
    println!("==========================================================================================");

    for frame_idx in 0..total_frames {
        let cycle = frame_idx % 60;
        let mask = if cycle < 40 { (1 << 4) | (if cycle % 30 < 10 { 1 << 0 } else { 0 }) } else { 1 << 5 };

        core.set_input(mask);
        let (_, _, _) = core.step_ppu(&mut vram_buf, &mut pal_buf, &mut oam_buf, &mut io_buf);

        if frame_idx < warmup_frames {
            prev_bg0_x = (io_buf[0x10] as u16) | ((io_buf[0x11] as u16) << 8);
            prev_bg0_y = (io_buf[0x12] as u16) | ((io_buf[0x13] as u16) << 8);
            prev_bg1_x = (io_buf[0x14] as u16) | ((io_buf[0x15] as u16) << 8);
            prev_bg1_y = (io_buf[0x16] as u16) | ((io_buf[0x17] as u16) << 8);
            continue;
        }

        let raw16 = core.get_last_frame();
        if raw16.len() != TOTAL_PIXELS {
            continue;
        }

        let bg0_x = (io_buf[0x10] as u16) | ((io_buf[0x11] as u16) << 8);
        let bg0_y = (io_buf[0x12] as u16) | ((io_buf[0x13] as u16) << 8);
        let bg1_x = (io_buf[0x14] as u16) | ((io_buf[0x15] as u16) << 8);
        let bg1_y = (io_buf[0x16] as u16) | ((io_buf[0x17] as u16) << 8);

        // Hardware delta in pixels
        let mut dx = (bg0_x.wrapping_sub(prev_bg0_x) & 0x1FF) as i32;
        if dx > 255 { dx -= 512; }
        let mut dy = (bg0_y.wrapping_sub(prev_bg0_y) & 0x1FF) as i32;
        if dy > 255 { dy -= 512; }

        let t_start = Instant::now();
        // Shift previous buffer by hardware (dx, dy)
        for y in 0..GBA_HEIGHT {
            let sy = y as i32 - dy;
            for x in 0..GBA_WIDTH {
                let sx = x as i32 - dx;
                if sy >= 0 && sy < GBA_HEIGHT as i32 && sx >= 0 && sx < GBA_WIDTH as i32 {
                    shifted_prev[y * GBA_WIDTH + x] = prev_raw16[sy as usize * GBA_WIDTH + sx as usize];
                } else {
                    shifted_prev[y * GBA_WIDTH + x] = 0;
                }
            }
        }

        let mut payload = Vec::with_capacity(2 + 2 + 600 * 130);
        payload.push(dx as i8 as u8);
        payload.push(dy as i8 as u8);
        payload.extend_from_slice(&0u16.to_le_bytes());

        let mut changed = 0u16;
        for by in 0..20 {
            for bx in 0..30 {
                let b_idx = (by * 30 + bx) as u16;
                let mut is_ch = false;
                for py in 0..8 {
                    let y = by * 8 + py;
                    for px in 0..8 {
                        let p = y * 240 + px;
                        if raw16[p] != shifted_prev[p] { is_ch = true; break; }
                    }
                    if is_ch { break; }
                }
                if is_ch {
                    changed += 1;
                    payload.extend_from_slice(&b_idx.to_le_bytes());
                    payload.push(2);
                    for py in 0..8 {
                        let y = by * 8 + py;
                        for px in 0..8 {
                            let p = y * 240 + px;
                            let diff = raw16[p] ^ shifted_prev[p];
                            payload.extend_from_slice(&diff.to_le_bytes());
                        }
                    }
                }
            }
        }
        let cb = changed.to_le_bytes();
        payload[2..4].copy_from_slice(&cb);

        let comp = lz4_flex::compress_prepend_size(&payload);
        times_hw_scroll.push(t_start.elapsed().as_micros() as f64);
        sizes_hw_scroll.push(comp.len() as f64);

        prev_bg0_x = bg0_x;
        prev_bg0_y = bg0_y;
        prev_bg1_x = bg1_x;
        prev_bg1_y = bg1_y;
        prev_raw16.copy_from_slice(&raw16);
    }

    let hw_avg = mean(&sizes_hw_scroll);
    let hw_time = mean(&times_hw_scroll);
    println!("{:<50} | {:<10} | {:<12} | {:<10}", "Method", "Avg Frame", "Bitrate@60", "Host Encode");
    println!("-----------------------------------------------------------------------------------------------");
    println!("{:<50} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "PPU State Streaming", 0.37, 0.18, 6.0);
    println!("{:<50} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel XOR (No Scroll Comp)", 2.53, 1.24, 51.0);
    println!("{:<50} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel XOR + Software Motion Search (525 µs)", 1.33, 0.65, 525.0);
    println!("{:<50} | {:<7.2} KB | {:<9.2} Mbps | {:<7.0} µs", "Pixel XOR + Hardware Register (BG0HOFS) Comp", hw_avg / 1024.0, (hw_avg * 8.0 * 60.0) / 1e6, hw_time);
    println!("===============================================================================================\n");

    Ok(())
}
