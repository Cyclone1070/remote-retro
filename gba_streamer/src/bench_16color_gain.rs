const TOTAL_PIXELS: usize = 240 * 160;

fn main() {
    println!("===================================================================");
    println!(" ⚡ 16-COLOR RETRO GBA SCENE BENCHMARK (4-BIT vs 8-BIT GAIN)");
    println!("===================================================================");
    println!("  Workload: 16 Unique Colors (e.g. Classic GameBoy, Tetris, Pong, Dialogues)");
    println!("-------------------------------------------------------------------");

    let mut palette = [0u16; 16];
    for i in 0..16 { palette[i] = (i * 2000) as u16; }

    let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];
    for ty in 0..20 {
        for tx in 0..30 {
            let tile_col = (ty * 30 + tx) % 16;
            for y in 0..8 {
                for x in 0..8 {
                    let idx = (ty * 8 + y) * 240 + (tx * 8 + x);
                    indexed_pixels[idx] = if (x + y) % 2 == 0 { 0 } else { tile_col as u8 };
                }
            }
        }
    }

    // 1. Standard 8-Bit Palette
    let mut payload_8bit = Vec::with_capacity(2 + 32 + TOTAL_PIXELS);
    payload_8bit.extend_from_slice(&(16u16).to_le_bytes());
    for c in &palette { payload_8bit.extend_from_slice(&c.to_le_bytes()); }
    payload_8bit.extend_from_slice(&indexed_pixels);
    let c_8bit = lz4_flex::compress_prepend_size(&payload_8bit);

    // 2. 4-Bit Nibble Packed
    let mut nibbles = vec![0u8; TOTAL_PIXELS / 2];
    for i in 0..TOTAL_PIXELS / 2 {
        nibbles[i] = (indexed_pixels[i * 2] & 0x0F) | ((indexed_pixels[i * 2 + 1] & 0x0F) << 4);
    }
    let mut payload_4bit = Vec::with_capacity(2 + 32 + TOTAL_PIXELS / 2);
    payload_4bit.extend_from_slice(&(16u16).to_le_bytes());
    for c in &palette { payload_4bit.extend_from_slice(&c.to_le_bytes()); }
    payload_4bit.extend_from_slice(&nibbles);
    let c_4bit = lz4_flex::compress_prepend_size(&payload_4bit);

    let sz_8 = c_8bit.len() as f64;
    let sz_4 = c_4bit.len() as f64;

    println!("{:<32} | {:<10} | {:<12}", "Encoding Format", "Frame Size", "Bitrate @ 60 FPS");
    println!("-------------------------------------------------------------------");
    println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps", "1. Standard 8-Bit Palette", sz_8 / 1024.0, (sz_8 * 8.0 * 60.0) / 1_000_000.0);
    println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps", "2. 4-Bit Nibble-Packed Palette", sz_4 / 1024.0, (sz_4 * 8.0 * 60.0) / 1_000_000.0);
    println!("-------------------------------------------------------------------");
    println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps (-{:.1}% size reduction)",
        "DELTA GAIN (4-Bit vs 8-Bit):", (sz_4 - sz_8) / 1024.0, (sz_4 - sz_8) * 8.0 * 60.0 / 1_000_000.0,
        (1.0 - sz_4 / sz_8) * 100.0);
    println!("===================================================================\n");
}
