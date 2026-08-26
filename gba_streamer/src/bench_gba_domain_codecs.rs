use std::collections::HashMap;
use std::time::Instant;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT; // 38,400

fn main() {
    println!("===================================================================");
    println!(" ⚡ DOMAIN-SPECIFIC GBA FRAME COMPRESSION BENCHMARK ");
    println!("===================================================================");
    println!("  Display Resolution: 240x160 (38,400 pixels)");
    println!("  Native Color Space: 15-bit RGB555 (0bbbbbgggggrrrrr, 32,768 colors)");
    println!("  Raw 16-bit Size:    {} Bytes (75.0 KiB)", TOTAL_PIXELS * 2);
    println!("-------------------------------------------------------------------");

    // Generate realistic GBA frame (64 unique 15-bit palette colors in 8x8 tiles)
    let mut palette = [0u16; 64];
    for i in 0..64 {
        let r = (i * 7) % 32;
        let g = (i * 13) % 32;
        let b = (i * 19) % 32;
        palette[i] = ((b << 10) | (g << 5) | r) as u16;
    }

    let mut raw16 = vec![0u16; TOTAL_PIXELS];
    for ty in 0..20 {
        for tx in 0..30 {
            let tile_color = palette[(ty * 30 + tx) % 64];
            for y in 0..8 {
                for x in 0..8 {
                    let idx = (ty * 8 + y) * GBA_WIDTH + (tx * 8 + x);
                    raw16[idx] = if (x + y) % 4 == 0 { palette[0] } else { tile_color };
                }
            }
        }
    }

    let raw_bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(raw16.as_ptr() as *const u8, TOTAL_PIXELS * 2)
    };

    // 1. Baseline: Raw 16-bit Interleaved + LZ4
    let t0 = Instant::now();
    let c_raw = lz4_flex::compress_prepend_size(raw_bytes);
    let t_raw = t0.elapsed().as_micros() as f64 / 1000.0;

    // 2. Planar Byte Separation (Low Byte Plane + High Byte Plane)
    // In RGB555: Low bytes (G+R) and High bytes (B+G) have massive horizontal spatial correlation
    let t0 = Instant::now();
    let mut planar_bytes = vec![0u8; TOTAL_PIXELS * 2];
    for i in 0..TOTAL_PIXELS {
        planar_bytes[i] = (raw16[i] & 0xFF) as u8; // Low byte plane
        planar_bytes[TOTAL_PIXELS + i] = ((raw16[i] >> 8) & 0xFF) as u8; // High byte plane
    }
    let c_planar = lz4_flex::compress_prepend_size(&planar_bytes);
    let t_planar = t0.elapsed().as_micros() as f64 / 1000.0;

    // 3. Dynamic Frame Palette (8-Bit Indexed + Palette Table)
    // Since GBA games use 256 or fewer colors per frame, map 16-bit pixels -> 8-bit index
    let t0 = Instant::now();
    let mut color_map: HashMap<u16, u8> = HashMap::with_capacity(256);
    let mut pal_table: Vec<u16> = Vec::with_capacity(256);
    let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];

    for i in 0..TOTAL_PIXELS {
        let c = raw16[i];
        let next_idx = pal_table.len() as u8;
        let idx = *color_map.entry(c).or_insert_with(|| {
            pal_table.push(c);
            next_idx
        });
        indexed_pixels[i] = idx;
    }

    let mut pal_payload = Vec::with_capacity(2 + pal_table.len() * 2 + TOTAL_PIXELS);
    pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
    for c in &pal_table {
        pal_payload.extend_from_slice(&c.to_le_bytes());
    }
    pal_payload.extend_from_slice(&indexed_pixels);
    let c_palette = lz4_flex::compress_prepend_size(&pal_payload);
    let t_palette = t0.elapsed().as_micros() as f64 / 1000.0;

    println!("{:<36} | {:<10} | {:<12} | {:<12} | {:<10}",
        "Compression Strategy", "Payload", "Bitrate@60", "Encode Time", "Reduction");
    println!("--------------------------------------------------------------------------------------------------");
    println!("{:<36} | {:<7.2} KB | {:<9.2} Mbps | {:<9.3} ms | Baseline",
        "1. Raw 16-Bit RGB555 + LZ4", c_raw.len() as f64 / 1024.0, (c_raw.len() as f64 * 8.0 * 60.0) / 1_000_000.0, t_raw);
    println!("{:<36} | {:<7.2} KB | {:<9.2} Mbps | {:<9.3} ms | -{:.1}% smaller",
        "2. Planar Byte-Split + LZ4", c_planar.len() as f64 / 1024.0, (c_planar.len() as f64 * 8.0 * 60.0) / 1_000_000.0, t_planar,
        (1.0 - c_planar.len() as f64 / c_raw.len() as f64) * 100.0);
    println!("{:<36} | {:<7.2} KB | {:<9.2} Mbps | {:<9.3} ms | -{:.1}% smaller",
        "3. 8-Bit Dynamic Palette + LZ4", c_palette.len() as f64 / 1024.0, (c_palette.len() as f64 * 8.0 * 60.0) / 1_000_000.0, t_palette,
        (1.0 - c_palette.len() as f64 / c_raw.len() as f64) * 100.0);
    println!("===================================================================\n");
}
