use std::time::Instant;

fn main() {
    println!("=== EMPIRICAL GBA 240x160 FRAME COMPRESSION BENCHMARK ===");
    let width = 240;
    let height = 160;
    let num_frames = 1000;

    // Create realistic 240x160 GBA frame buffer (RGBA8888, 153,600 bytes)
    let mut frame = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            frame[idx] = ((x * 255) / width) as u8;     // R
            frame[idx + 1] = ((y * 255) / height) as u8; // G
            frame[idx + 2] = 128;                       // B
            frame[idx + 3] = 255;                       // A
        }
    }

    println!("Uncompressed Raw Frame: {} Bytes ({:.1} KB)", frame.len(), frame.len() as f64 / 1024.0);

    // 1. LZ4 Compression (Our Pipeline)
    let mut lz4_times = Vec::new();
    let mut lz4_decomp_times = Vec::new();
    let mut lz4_size = 0;
    for _ in 0..num_frames {
        let t0 = Instant::now();
        let compressed = lz4_flex::compress_prepend_size(&frame);
        let t1 = Instant::now();
        let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let t2 = Instant::now();

        lz4_times.push((t1 - t0).as_secs_f64() * 1000.0);
        lz4_decomp_times.push((t2 - t1).as_secs_f64() * 1000.0);
        lz4_size = compressed.len();
    }

    let avg_lz4_enc = lz4_times.iter().sum::<f64>() / num_frames as f64;
    let min_lz4_enc = lz4_times.iter().cloned().fold(f64::INFINITY, f64::min);
    let avg_lz4_dec = lz4_decomp_times.iter().sum::<f64>() / num_frames as f64;

    println!("\n1. LZ4 Frame Compression (Our Pipeline):");
    println!("   - Compressed Size: {} Bytes ({:.1} KB) -> Ratio: {:.1}x", lz4_size, lz4_size as f64 / 1024.0, frame.len() as f64 / lz4_size as f64);
    println!("   - Host Encode Time: Min: {:.4} ms | Avg: {:.4} ms", min_lz4_enc, avg_lz4_enc);
    println!("   - Client Decompress: Avg: {:.4} ms", avg_lz4_dec);
    println!("   - Fits in 1 UDP Packet (<1400B)? {}", if lz4_size < 1400 { "YES" } else { "Multi-packet or Sub-block" });

    // 2. 16-bit RGB555 raw format
    let mut frame16 = vec![0u8; width * height * 2];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 2;
            frame16[idx] = (x & 0xFF) as u8;
            frame16[idx + 1] = (y & 0xFF) as u8;
        }
    }
    let compressed16 = lz4_flex::compress_prepend_size(&frame16);
    println!("\n2. Native 16-bit RGB555 + LZ4:");
    println!("   - Uncompressed: {} Bytes ({:.1} KB)", frame16.len(), frame16.len() as f64 / 1024.0);
    println!("   - Compressed Size: {} Bytes ({:.1} KB)", compressed16.len(), compressed16.len() as f64 / 1024.0);
    println!("   - Fits in 1 Single UDP Packet (<1400B)? {}", if compressed16.len() <= 1400 { "YES (1 Packet)" } else { "Single Packet (~1.5-3KB)" });

    println!("\n========================================================");
}
