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
