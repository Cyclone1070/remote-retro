use anyhow::{Context, Result};
use gba_streamer::codec::{PaletteEncoder, TOTAL_PIXELS};
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

fn main() -> Result<()> {
    let core_path = "/usr/lib64/libretro/mgba_libretro.so";
    let rom_path = "/tmp/stuntman.gba";
    let total_frames: usize = 4700;
    let warmup_frames: usize = 3200;

    let mut core = RetroCore::load(core_path, rom_path)
        .context("Failed to load core or ROM")?;

    let mut pal_delta_enc = PaletteEncoder::new();

    let mut vram_buf = vec![0u8; 98304];
    let mut pal_buf = vec![0u8; 1024];
    let mut oam_buf = vec![0u8; 1024];
    let mut io_buf = vec![0u8; 128];

    // Candidate Optimizations for Stuntman:
    // 1. Current Baseline: 16-bit RGB Tile Delta (Raw blocks)
    let mut sizes_baseline = Vec::with_capacity(1500);

    // 2. 16-bit RGB Tile Delta with XOR (Difference blocks)
    let mut sizes_rgb_tile_xor = Vec::with_capacity(1500);

    // 3. 16-bit Full Frame Temporal XOR + LZ4
    let mut sizes_rgb_frame_xor = Vec::with_capacity(1500);

    // 4. Native Mode 4: 8-Bit Active Page LZ4 (38.4 KB raw)
    let mut sizes_m4_raw = Vec::with_capacity(1500);

    // 5. Native Mode 4: Double-Buffer Page-Aware XOR Delta + LZ4
    let mut sizes_m4_xor = Vec::with_capacity(1500);

    // 6. Native Mode 4: 8-Bit 8x8 Tile Delta (Raw blocks)
    let mut sizes_m4_tile_raw = Vec::with_capacity(1500);

    // 7. Native Mode 4: 8-Bit 8x8 Tile Delta (XOR blocks)
    let mut sizes_m4_tile_xor = Vec::with_capacity(1500);

    // 8. Native Mode 4: Double-Buffered XOR (comparing page 0 to prev page 0, page 1 to prev page 1)
    let mut sizes_m4_db_xor = Vec::with_capacity(1500);

    let mut time_baseline = Vec::with_capacity(1500);
    let mut time_rgb_xor = Vec::with_capacity(1500);

    let mut prev_raw16 = vec![0u16; TOTAL_PIXELS];
    let mut prev_displayed_m4 = vec![0u8; 38400];
    let mut prev_m4_page0 = vec![0u8; 38400];
    let mut prev_m4_page1 = vec![0u8; 38400];
    let mut m4_tile_prev = vec![0u8; 38400];

    println!("==========================================================================================");
    println!(" 🚀 EXPLORING CANDIDATE OPTIMIZATIONS FOR STUNTMAN (3D GBA) - 1,500 DRIVING FRAMES");
    println!("==========================================================================================");

    for frame_idx in 0..total_frames {
        let cycle = frame_idx % 50;
        let mask = if frame_idx < warmup_frames {
            if cycle < 10 { 1 << 0 } else if cycle >= 25 && cycle < 35 { 1 << 3 } else { 0 }
        } else {
            let f = frame_idx - warmup_frames;
            let mut m = 1 << 0;
            if (f / 35) % 2 == 0 { m |= 1 << 4; } else { m |= 1 << 5; }
            if f % 120 > 90 { m |= 1 << 1; }
            m
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

        // 1. Current Baseline: 16-bit RGB Tile Delta (Raw blocks)
        let t0 = std::time::Instant::now();
        let (_, b_delta) = pal_delta_enc.encode(&raw16);
        time_baseline.push(t0.elapsed().as_micros() as f64);
        sizes_baseline.push(b_delta.len() as f64);

        // 2. 16-bit RGB Tile Delta with XOR
        let t1 = std::time::Instant::now();
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
            time_rgb_xor.push(t1.elapsed().as_micros() as f64);
            sizes_rgb_tile_xor.push(comp.len() as f64);
        }

        // 3. 16-bit Full Frame Temporal XOR
        {
            let mut diff_bytes = Vec::with_capacity(TOTAL_PIXELS * 2);
            for p in 0..TOTAL_PIXELS {
                let d = raw16[p] ^ prev_raw16[p];
                diff_bytes.extend_from_slice(&d.to_le_bytes());
            }
            let comp = lz4_flex::compress_prepend_size(&diff_bytes);
            sizes_rgb_frame_xor.push(comp.len() as f64);
        }
        prev_raw16.copy_from_slice(&raw16);

        // Mode 4 Active Page
        let dispcnt = (io_buf[0] as u16) | ((io_buf[1] as u16) << 8);
        let is_page1 = (dispcnt & (1 << 4)) != 0;
        let page_offset = if is_page1 { 0xA000 } else { 0x0000 };
        let active_page = &vram_buf[page_offset..page_offset + 38400];

        // 4. Native Mode 4: 8-Bit Active Page LZ4
        let c_m4_raw = lz4_flex::compress_prepend_size(active_page);
        sizes_m4_raw.push(c_m4_raw.len() as f64);

        // 5. Native Mode 4: Temporal XOR Delta against previous displayed page
        {
            let mut xor_buf = [0u8; 38400];
            for i in 0..38400 {
                xor_buf[i] = active_page[i] ^ prev_displayed_m4[i];
            }
            let c = lz4_flex::compress_prepend_size(&xor_buf);
            sizes_m4_xor.push(c.len() as f64);
            prev_displayed_m4.copy_from_slice(active_page);
        }

        // 6 & 7. Native Mode 4: 8x8 Tile Delta (Raw & XOR)
        {
            let mut ch_raw = 0u16;
            let mut p_raw = Vec::with_capacity(2 + 600 * 66);
            p_raw.extend_from_slice(&0u16.to_le_bytes());

            let mut ch_xor = 0u16;
            let mut p_xor = Vec::with_capacity(2 + 600 * 66);
            p_xor.extend_from_slice(&0u16.to_le_bytes());

            for by in 0..20 {
                for bx in 0..30 {
                    let b_idx = (by * 30 + bx) as u16;
                    let mut changed = false;
                    for py in 0..8 {
                        let y = by * 8 + py;
                        for px in 0..8 {
                            let p = y * 240 + px;
                            if active_page[p] != m4_tile_prev[p] { changed = true; break; }
                        }
                        if changed { break; }
                    }

                    if changed {
                        ch_raw += 1;
                        p_raw.extend_from_slice(&b_idx.to_le_bytes());
                        p_raw.push(1);
                        ch_xor += 1;
                        p_xor.extend_from_slice(&b_idx.to_le_bytes());
                        p_xor.push(1);

                        for py in 0..8 {
                            let y = by * 8 + py;
                            for px in 0..8 {
                                let p = y * 240 + px;
                                p_raw.push(active_page[p]);
                                p_xor.push(active_page[p] ^ m4_tile_prev[p]);
                            }
                        }
                    }
                }
            }
            let r_b = ch_raw.to_le_bytes();
            p_raw[0..2].copy_from_slice(&r_b);
            let x_b = ch_xor.to_le_bytes();
            p_xor[0..2].copy_from_slice(&x_b);

            m4_tile_prev.copy_from_slice(active_page);
            sizes_m4_tile_raw.push(lz4_flex::compress_prepend_size(&p_raw).len() as f64);
            sizes_m4_tile_xor.push(lz4_flex::compress_prepend_size(&p_xor).len() as f64);
        }

        // 8. Native Mode 4: Double-Buffered XOR (page matching)
        {
            let prev_target = if is_page1 { &prev_m4_page1 } else { &prev_m4_page0 };
            let mut db_xor_buf = [0u8; 38400];
            for i in 0..38400 {
                db_xor_buf[i] = active_page[i] ^ prev_target[i];
            }
            let c = lz4_flex::compress_prepend_size(&db_xor_buf);
            sizes_m4_db_xor.push(c.len() as f64);

            if is_page1 {
                prev_m4_page1.copy_from_slice(active_page);
            } else {
                prev_m4_page0.copy_from_slice(active_page);
            }
        }
    }

    println!("{:<50} | {:<10} | {:<12} | {:<10} | {:<10}",
        "Optimization Strategy", "Avg Frame", "Bitrate@60", "P95 Frame", "Encode µs");
    println!("------------------------------------------------------------------------------------------------------");

    let print_row = |name: &str, sizes: &mut [f64], encode_us: Option<f64>| {
        let avg_sz = mean(sizes);
        let mbps = (avg_sz * 8.0 * 60.0) / 1_000_000.0;
        let p95_sz = percentile(sizes, 0.95);
        let enc_str = encode_us.map(|u| format!("{:.0} µs", u)).unwrap_or_else(|| "-".to_string());
        println!("{:<50} | {:<7.2} KB | {:<9.2} Mbps | {:<7.2} KB | {:<10}",
            name, avg_sz / 1024.0, mbps, p95_sz / 1024.0, enc_str);
    };

    print_row("Baseline: 16-bit RGB Tile Delta (Raw blocks)", &mut sizes_baseline, Some(mean(&time_baseline)));
    print_row("Candidate 1: 16-bit RGB Tile Delta (XOR blocks)", &mut sizes_rgb_tile_xor, Some(mean(&time_rgb_xor)));
    print_row("Candidate 2: 16-bit Full Frame Temporal XOR", &mut sizes_rgb_frame_xor, None);
    print_row("Candidate 3: Native Mode 4 8-bit Page LZ4", &mut sizes_m4_raw, None);
    print_row("Candidate 4: Native Mode 4 Temporal XOR Delta", &mut sizes_m4_xor, None);
    print_row("Candidate 5: Native Mode 4 8-bit Tile Delta (Raw)", &mut sizes_m4_tile_raw, None);
    print_row("Candidate 6: Native Mode 4 8-bit Tile Delta (XOR)", &mut sizes_m4_tile_xor, None);
    print_row("Candidate 7: Native Mode 4 Double-Buffer Page XOR", &mut sizes_m4_db_xor, None);
    println!("======================================================================================================\n");

    Ok(())
}
