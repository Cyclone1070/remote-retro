use std::collections::HashMap;

pub const GBA_WIDTH: usize = 240;
pub const GBA_HEIGHT: usize = 160;
pub const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;

pub const BLOCK_SIZE: usize = 8;
pub const BLOCKS_X: usize = GBA_WIDTH / BLOCK_SIZE; // 30
pub const BLOCKS_Y: usize = GBA_HEIGHT / BLOCK_SIZE; // 20
pub const TOTAL_BLOCKS: usize = BLOCKS_X * BLOCKS_Y; // 600
pub const BLOCK_PIXELS: usize = BLOCK_SIZE * BLOCK_SIZE; // 64

pub struct PaletteEncoder {
    color_map: HashMap<u16, u8>,
    pal_table: Vec<u16>,
    indexed_pixels: Vec<u8>,
    nibble_packed: Vec<u8>,
    pal_payload: Vec<u8>,
    prev_frame: Vec<u16>,
    delta_payload: Vec<u8>,
    frame_counter: u32,
}

impl PaletteEncoder {
    pub fn new() -> Self {
        Self {
            color_map: HashMap::with_capacity(256),
            pal_table: Vec::with_capacity(256),
            indexed_pixels: vec![0u8; TOTAL_PIXELS],
            nibble_packed: vec![0u8; TOTAL_PIXELS / 2],
            pal_payload: Vec::with_capacity(2 + 512 + TOTAL_PIXELS),
            prev_frame: vec![0u16; TOTAL_PIXELS],
            delta_payload: Vec::with_capacity(2 + TOTAL_BLOCKS * (2 + BLOCK_PIXELS * 2)),
            frame_counter: 0,
        }
    }

    pub fn encode(&mut self, raw_frame: &[u16]) -> (u8, Vec<u8>) {
        self.frame_counter += 1;

        // Try Delta Block encoding on inter-frames (except keyframe every 60 frames)
        if self.frame_counter % 60 != 1 && !self.prev_frame.is_empty() {
            let mut changed_blocks = 0u16;
            self.delta_payload.clear();
            self.delta_payload.extend_from_slice(&0u16.to_le_bytes()); // placeholder for count

            for by in 0..BLOCKS_Y {
                for bx in 0..BLOCKS_X {
                    let b_idx = (by * BLOCKS_X + bx) as u16;
                    let mut is_changed = false;

                    for py in 0..BLOCK_SIZE {
                        let y = by * BLOCK_SIZE + py;
                        for px in 0..BLOCK_SIZE {
                            let x = bx * BLOCK_SIZE + px;
                            let p = y * GBA_WIDTH + x;
                            if raw_frame[p] != self.prev_frame[p] {
                                is_changed = true;
                                break;
                            }
                        }
                        if is_changed { break; }
                    }

                    if is_changed {
                        changed_blocks += 1;
                        self.delta_payload.extend_from_slice(&b_idx.to_le_bytes());
                        for py in 0..BLOCK_SIZE {
                            let y = by * BLOCK_SIZE + py;
                            for px in 0..BLOCK_SIZE {
                                let x = bx * BLOCK_SIZE + px;
                                let p = y * GBA_WIDTH + x;
                                self.delta_payload.extend_from_slice(&raw_frame[p].to_le_bytes());
                            }
                        }
                    }
                }
            }

            // If changes are compact (<= 250 blocks out of 600, or 0 if unchanged)
            if changed_blocks <= 250 {
                let count_bytes = changed_blocks.to_le_bytes();
                self.delta_payload[0..2].copy_from_slice(&count_bytes);
                self.prev_frame.copy_from_slice(raw_frame);
                return (8u8, lz4_flex::compress_prepend_size(&self.delta_payload));
            }
        }

        // Full Keyframe Palette Encoding
        self.prev_frame.copy_from_slice(raw_frame);
        self.color_map.clear();
        self.pal_table.clear();

        let mut fits_256 = true;
        for p in 0..TOTAL_PIXELS {
            let c = raw_frame[p];
            if let Some(&idx) = self.color_map.get(&c) {
                self.indexed_pixels[p] = idx;
            } else if self.pal_table.len() < 256 {
                let idx = self.pal_table.len() as u8;
                self.color_map.insert(c, idx);
                self.pal_table.push(c);
                self.indexed_pixels[p] = idx;
            } else {
                fits_256 = false;
                break;
            }
        }

        if fits_256 && self.pal_table.len() <= 16 {
            for i in 0..TOTAL_PIXELS / 2 {
                self.nibble_packed[i] =
                    (self.indexed_pixels[i * 2] & 0x0F) | ((self.indexed_pixels[i * 2 + 1] & 0x0F) << 4);
            }
            self.pal_payload.clear();
            self.pal_payload.extend_from_slice(&(self.pal_table.len() as u16).to_le_bytes());
            for c in &self.pal_table {
                self.pal_payload.extend_from_slice(&c.to_le_bytes());
            }
            self.pal_payload.extend_from_slice(&self.nibble_packed);
            (4u8, lz4_flex::compress_prepend_size(&self.pal_payload))
        } else if fits_256 {
            self.pal_payload.clear();
            self.pal_payload.extend_from_slice(&(self.pal_table.len() as u16).to_le_bytes());
            for c in &self.pal_table {
                self.pal_payload.extend_from_slice(&c.to_le_bytes());
            }
            self.pal_payload.extend_from_slice(&self.indexed_pixels);
            (2u8, lz4_flex::compress_prepend_size(&self.pal_payload))
        } else {
            let byte_slice = unsafe {
                std::slice::from_raw_parts(raw_frame.as_ptr() as *const u8, raw_frame.len() * 2)
            };
            (1u8, lz4_flex::compress_prepend_size(byte_slice))
        }
    }
}

pub struct AudioEncoder {
    pub sample_rate: u32,
    buffer: Vec<i16>,
}

impl AudioEncoder {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            buffer: Vec::with_capacity(4096),
        }
    }

    pub fn push_samples(&mut self, samples: &[i16]) {
        self.buffer.extend_from_slice(samples);
    }

    pub fn flush_frame_lz4(&mut self) -> Option<Vec<u8>> {
        if self.buffer.is_empty() {
            return None;
        }
        let byte_slice = unsafe {
            std::slice::from_raw_parts(self.buffer.as_ptr() as *const u8, self.buffer.len() * 2)
        };
        let compressed = lz4_flex::compress_prepend_size(byte_slice);
        self.buffer.clear();
        Some(compressed)
    }
}

pub const FLAG_PPU_STATE: u8 = 16; // 0x10

pub struct PpuStateEncoder {
    prev_vram: Vec<u8>,
    prev_palette: Vec<u8>,
    frame_counter: u32,
}

impl PpuStateEncoder {
    pub fn new() -> Self {
        Self {
            prev_vram: vec![0u8; 98304],
            prev_palette: vec![0u8; 1024],
            frame_counter: 0,
        }
    }

    pub fn encode(
        &mut self,
        oam: &[u8],      // 1024 bytes
        io: &[u8],       // 128 bytes
        palette: &[u8],  // 1024 bytes
        vram: &[u8],     // 98304 bytes
    ) -> Vec<u8> {
        self.frame_counter += 1;

        let oam_comp = lz4_flex::compress_prepend_size(oam);
        let io_comp = lz4_flex::compress_prepend_size(io);

        let pal_comp = if self.frame_counter % 60 == 1 || palette != self.prev_palette.as_slice() {
            self.prev_palette.copy_from_slice(palette);
            lz4_flex::compress_prepend_size(palette)
        } else {
            Vec::new()
        };

        const BLOCK_SZ: usize = 128;
        const NUM_BLOCKS: usize = 98304 / BLOCK_SZ; // 768

        let is_keyframe = self.frame_counter % 120 == 1;
        let mut dirty_blocks: Vec<u16> = Vec::new();

        if !is_keyframe {
            for b in 0..NUM_BLOCKS {
                let start = b * BLOCK_SZ;
                let end = start + BLOCK_SZ;
                if vram[start..end] != self.prev_vram[start..end] {
                    dirty_blocks.push(b as u16);
                }
            }
        }

        let (vram_mode, vram_payload) = if is_keyframe || dirty_blocks.len() > 150 {
            self.prev_vram.copy_from_slice(vram);
            let comp = lz4_flex::compress_prepend_size(vram);
            (1u8, comp)
        } else if !dirty_blocks.is_empty() {
            let mut delta = Vec::with_capacity(2 + dirty_blocks.len() * (2 + BLOCK_SZ));
            delta.extend_from_slice(&(dirty_blocks.len() as u16).to_le_bytes());
            for &b in &dirty_blocks {
                delta.extend_from_slice(&b.to_le_bytes());
                let start = b as usize * BLOCK_SZ;
                let end = start + BLOCK_SZ;
                delta.extend_from_slice(&vram[start..end]);
                self.prev_vram[start..end].copy_from_slice(&vram[start..end]);
            }
            let comp = lz4_flex::compress_prepend_size(&delta);
            (2u8, comp)
        } else {
            (0u8, Vec::new())
        };

        let mut out = Vec::with_capacity(16 + oam_comp.len() + io_comp.len() + pal_comp.len() + vram_payload.len());
        out.extend_from_slice(&(oam_comp.len() as u16).to_le_bytes());
        out.extend_from_slice(&oam_comp);

        out.extend_from_slice(&(io_comp.len() as u16).to_le_bytes());
        out.extend_from_slice(&io_comp);

        out.extend_from_slice(&(pal_comp.len() as u16).to_le_bytes());
        out.extend_from_slice(&pal_comp);

        let vram_section_len = (1 + vram_payload.len()) as u32;
        out.extend_from_slice(&vram_section_len.to_le_bytes());
        out.push(vram_mode);
        out.extend_from_slice(&vram_payload);

        out
    }
}

pub struct PpuState {
    pub oam: [u8; 1024],
    pub io: [u8; 128],
    pub palette: [u8; 1024],
    pub vram: [u8; 98304],
}

impl PpuState {
    pub fn new() -> Self {
        Self {
            oam: [0u8; 1024],
            io: [0u8; 128],
            palette: [0u8; 1024],
            vram: [0u8; 98304],
        }
    }

    pub fn apply_payload(&mut self, payload: &[u8]) -> Result<(), String> {
        if payload.len() < 10 {
            return Err("Payload too short for header".into());
        }
        let mut pos = 0;

        let oam_len = u16::from_le_bytes(payload[pos..pos+2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + oam_len > payload.len() { return Err("Invalid oam len".into()); }
        let decomp_oam = lz4_flex::decompress_size_prepended(&payload[pos..pos+oam_len])
            .map_err(|e| format!("LZ4 OAM: {}", e))?;
        pos += oam_len;
        if decomp_oam.len() == 1024 {
            self.oam.copy_from_slice(&decomp_oam);
        }

        if pos + 2 > payload.len() { return Err("Invalid io header".into()); }
        let io_len = u16::from_le_bytes(payload[pos..pos+2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + io_len > payload.len() { return Err("Invalid io len".into()); }
        let decomp_io = lz4_flex::decompress_size_prepended(&payload[pos..pos+io_len])
            .map_err(|e| format!("LZ4 IO: {}", e))?;
        pos += io_len;
        let copy_len = decomp_io.len().min(128);
        self.io[..copy_len].copy_from_slice(&decomp_io[..copy_len]);

        if pos + 2 > payload.len() { return Err("Invalid pal header".into()); }
        let pal_len = u16::from_le_bytes(payload[pos..pos+2].try_into().unwrap()) as usize;
        pos += 2;
        if pal_len > 0 {
            if pos + pal_len > payload.len() { return Err("Invalid pal len".into()); }
            let decomp_pal = lz4_flex::decompress_size_prepended(&payload[pos..pos+pal_len])
                .map_err(|e| format!("LZ4 Pal: {}", e))?;
            pos += pal_len;
            if decomp_pal.len() == 1024 {
                self.palette.copy_from_slice(&decomp_pal);
            }
        }

        if pos + 5 > payload.len() { return Err("Invalid vram header".into()); }
        let vram_section_len = u32::from_le_bytes(payload[pos..pos+4].try_into().unwrap()) as usize;
        pos += 4;
        let vram_mode = payload[pos];
        pos += 1;
        let vram_payload_len = vram_section_len.saturating_sub(1);
        if vram_mode == 1 && vram_payload_len > 0 {
            let decomp = lz4_flex::decompress_size_prepended(&payload[pos..pos+vram_payload_len])
                .map_err(|e| format!("LZ4 VRAM full: {}", e))?;
            if decomp.len() == 98304 {
                self.vram.copy_from_slice(&decomp);
            }
        } else if vram_mode == 2 && vram_payload_len > 0 {
            let decomp = lz4_flex::decompress_size_prepended(&payload[pos..pos+vram_payload_len])
                .map_err(|e| format!("LZ4 VRAM delta: {}", e))?;
            if decomp.len() >= 2 {
                let count = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                let mut d_pos = 2;
                for _ in 0..count {
                    if d_pos + 2 + 128 > decomp.len() { break; }
                    let b = u16::from_le_bytes(decomp[d_pos..d_pos+2].try_into().unwrap()) as usize;
                    d_pos += 2;
                    let start = b * 128;
                    let end = start + 128;
                    if end <= 98304 {
                        self.vram[start..end].copy_from_slice(&decomp[d_pos..d_pos+128]);
                    }
                    d_pos += 128;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_4bit_nibble_palette_roundtrip() {
        let mut encoder = PaletteEncoder::new();
        // 16 exact distinct colors (0..16) distributed across frame
        let mut frame = vec![0u16; TOTAL_PIXELS];
        for i in 0..TOTAL_PIXELS {
            frame[i] = (i % 16) as u16;
        }

        let (flag, compressed) = encoder.encode(&frame);
        assert_eq!(flag, 4, "Should select 4-bit nibble packing for <= 16 colors");

        let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
        assert_eq!(pal_len, 16);

        let pal_src = unsafe {
            std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
        };
        let packed = &decomp[2 + pal_len * 2..];
        assert_eq!(packed.len(), TOTAL_PIXELS / 2);

        let mut reconstructed = vec![0u16; TOTAL_PIXELS];
        for i in 0..TOTAL_PIXELS / 2 {
            let b = packed[i];
            reconstructed[i * 2] = pal_src[(b & 0x0F) as usize];
            reconstructed[i * 2 + 1] = pal_src[((b >> 4) & 0x0F) as usize];
        }

        assert_eq!(frame, reconstructed, "4-bit palette must be 100% bit-exact");
    }

    #[test]
    fn test_8bit_palette_roundtrip() {
        let mut encoder = PaletteEncoder::new();
        // 64 exact distinct colors (0..64) distributed across frame
        let mut frame = vec![0u16; TOTAL_PIXELS];
        for i in 0..TOTAL_PIXELS {
            frame[i] = (i % 64) as u16;
        }

        let (flag, compressed) = encoder.encode(&frame);
        assert_eq!(flag, 2, "Should select 8-bit palette for 17-256 colors");

        let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
        assert_eq!(pal_len, 64);

        let pal_src = unsafe {
            std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
        };
        let indices = &decomp[2 + pal_len * 2..];
        assert_eq!(indices.len(), TOTAL_PIXELS);

        let mut reconstructed = vec![0u16; TOTAL_PIXELS];
        for p in 0..TOTAL_PIXELS {
            reconstructed[p] = pal_src[indices[p] as usize];
        }

        assert_eq!(frame, reconstructed, "8-bit palette must be 100% bit-exact");
    }

    #[test]
    fn test_delta_tile_block_roundtrip() {
        let mut encoder = PaletteEncoder::new();
        let frame1 = vec![0x1234u16; TOTAL_PIXELS];
        let (flag1, _) = encoder.encode(&frame1);
        assert_eq!(flag1, 4, "Initial frame is keyframe");

        // Move a sprite in block 15
        let mut frame2 = frame1.clone();
        for py in 0..8 {
            for px in 0..8 {
                let p = py * GBA_WIDTH + (15 * 8 + px);
                frame2[p] = 0x5678u16;
            }
        }

        let (flag2, compressed) = encoder.encode(&frame2);
        assert_eq!(flag2, 8, "Small update must trigger Delta Block encoding");

        let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let num_blocks = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
        assert_eq!(num_blocks, 1, "Exactly 1 block should be modified");

        let block_idx = u16::from_le_bytes(decomp[2..4].try_into().unwrap()) as usize;
        assert_eq!(block_idx, 15);

        let mut reconstructed = frame1.clone();
        let bx = block_idx % BLOCKS_X;
        let by = block_idx / BLOCKS_X;
        let block_data = unsafe {
            std::slice::from_raw_parts(decomp[4..4 + BLOCK_PIXELS * 2].as_ptr() as *const u16, BLOCK_PIXELS)
        };

        for py in 0..BLOCK_SIZE {
            let y = by * BLOCK_SIZE + py;
            for px in 0..BLOCK_SIZE {
                let x = bx * BLOCK_SIZE + px;
                reconstructed[y * GBA_WIDTH + x] = block_data[py * BLOCK_SIZE + px];
            }
        }

        assert_eq!(frame2, reconstructed, "Delta Block decoding must be 100% bit-exact");
    }

    #[test]
    fn test_audio_chunking_and_lz4_roundtrip() {
        let mut audio_enc = AudioEncoder::new(44100);
        let samples: Vec<i16> = (0..735 * 2).map(|i| (i % 1000) as i16).collect();
        audio_enc.push_samples(&samples);

        let compressed = audio_enc.flush_frame_lz4().expect("Must yield compressed audio");
        assert!(compressed.len() < samples.len() * 2, "LZ4 must compress repetitive audio PCM");

        let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let decomp_i16 = unsafe {
            std::slice::from_raw_parts(decomp.as_ptr() as *const i16, decomp.len() / 2)
        };
        assert_eq!(samples, decomp_i16, "Audio PCM must be 100% lossless bit-exact");
    }

    #[test]
    fn test_ppu_state_encoder_roundtrip() {
        let mut encoder = PpuStateEncoder::new();
        let mut decoder = PpuState::new();

        let mut oam = vec![0u8; 1024];
        let mut io = vec![0u8; 128];
        let mut pal = vec![0u8; 1024];
        let mut vram = vec![0u8; 98304];

        // Seed some identifiable data
        oam[0] = 42;
        io[0] = 0x01; // DISPCNT
        pal[0] = 0x1F;
        vram[0] = 0xAA;

        // Frame 1: Keyframe
        let payload1 = encoder.encode(&oam, &io, &pal, &vram);
        assert!(decoder.apply_payload(&payload1).is_ok());
        assert_eq!(decoder.oam[0], 42);
        assert_eq!(decoder.io[0], 0x01);
        assert_eq!(decoder.palette[0], 0x1F);
        assert_eq!(decoder.vram[0], 0xAA);

        // Frame 2: Inter-frame with moved sprite in OAM, scrolled IO, unchanged VRAM & Palette
        oam[0] = 55;
        io[0x10] = 12; // BG0HOFS
        let payload2 = encoder.encode(&oam, &io, &pal, &vram);
        // Inter-frame must be ultra-compact (< 300 bytes)
        assert!(payload2.len() < 300, "Inter-frame PPU state must be < 300 bytes, got {}", payload2.len());

        assert!(decoder.apply_payload(&payload2).is_ok());
        assert_eq!(decoder.oam[0], 55);
        assert_eq!(decoder.io[0x10], 12);
        assert_eq!(decoder.palette[0], 0x1F);
        assert_eq!(decoder.vram[0], 0xAA);

        // Frame 3: Delta VRAM update (1 tile change)
        vram[128] = 0xEE;
        let payload3 = encoder.encode(&oam, &io, &pal, &vram);
        assert!(payload3.len() < 400, "Delta VRAM update should be < 400 bytes, got {}", payload3.len());

        assert!(decoder.apply_payload(&payload3).is_ok());
        assert_eq!(decoder.vram[128], 0xEE);
    }
}
