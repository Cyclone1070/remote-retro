use std::collections::HashMap;

pub const GBA_WIDTH: usize = 240;
pub const GBA_HEIGHT: usize = 160;
pub const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;

pub struct PaletteEncoder {
    color_map: HashMap<u16, u8>,
    pal_table: Vec<u16>,
    indexed_pixels: Vec<u8>,
    nibble_packed: Vec<u8>,
    pal_payload: Vec<u8>,
}

impl PaletteEncoder {
    pub fn new() -> Self {
        Self {
            color_map: HashMap::with_capacity(256),
            pal_table: Vec::with_capacity(256),
            indexed_pixels: vec![0u8; TOTAL_PIXELS],
            nibble_packed: vec![0u8; TOTAL_PIXELS / 2],
            pal_payload: Vec::with_capacity(2 + 512 + TOTAL_PIXELS),
        }
    }

    pub fn encode(&mut self, raw_frame: &[u16]) -> (u8, Vec<u8>) {
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
}
