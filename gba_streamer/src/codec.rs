use std::collections::HashMap;

pub const GBA_WIDTH: usize = 240;
pub const GBA_HEIGHT: usize = 160;
pub const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;

pub const BLOCK_SIZE: usize = 8;
pub const BLOCKS_X: usize = GBA_WIDTH / BLOCK_SIZE; // 30
pub const BLOCKS_Y: usize = GBA_HEIGHT / BLOCK_SIZE; // 20
pub const TOTAL_BLOCKS: usize = BLOCKS_X * BLOCKS_Y; // 600
pub const BLOCK_PIXELS: usize = BLOCK_SIZE * BLOCK_SIZE; // 64

pub const DICT_CACHE_SIZE: usize = 4096;

pub struct PaletteEncoder {
    color_map: HashMap<u16, u8>,
    pal_table: Vec<u16>,
    indexed_pixels: Vec<u8>,
    nibble_packed: Vec<u8>,
    pal_payload: Vec<u8>,
    prev_frame: Vec<u16>,
    delta_payload: Vec<u8>,
    tile_dict: HashMap<[u16; BLOCK_PIXELS], u16>,
    cache_head: u16,
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
            delta_payload: Vec::with_capacity(2 + TOTAL_BLOCKS * (3 + BLOCK_PIXELS * 2)),
            tile_dict: HashMap::with_capacity(DICT_CACHE_SIZE),
            cache_head: 0,
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

                        let mut tile = [0u16; BLOCK_PIXELS];
                        for py in 0..BLOCK_SIZE {
                            let y = by * BLOCK_SIZE + py;
                            for px in 0..BLOCK_SIZE {
                                let x = bx * BLOCK_SIZE + px;
                                tile[py * BLOCK_SIZE + px] = raw_frame[y * GBA_WIDTH + x];
                            }
                        }

                        // Mode 0: Check if solid color
                        let first = tile[0];
                        let is_solid = tile.iter().all(|&c| c == first);
                        if is_solid {
                            self.delta_payload.push(0); // Mode 0: Solid
                            self.delta_payload.extend_from_slice(&first.to_le_bytes());
                        } else if let Some(&cache_id) = self.tile_dict.get(&tile) {
                            // Mode 1: Dynamic Dictionary Cache Hit
                            self.delta_payload.push(1); // Mode 1: Cache Hit
                            self.delta_payload.extend_from_slice(&cache_id.to_le_bytes());
                        } else {
                            // Cache miss: assign next cache ID in FIFO ring
                            let cache_id = self.cache_head;
                            self.cache_head = (self.cache_head + 1) % (DICT_CACHE_SIZE as u16);
                            self.tile_dict.insert(tile, cache_id);

                            // Inspect palette size of tile
                            let mut ucolors = Vec::with_capacity(16);
                            let mut cmap = HashMap::with_capacity(16);
                            let mut fits_16 = true;

                            for &c in &tile {
                                if !cmap.contains_key(&c) {
                                    if ucolors.len() < 16 {
                                        cmap.insert(c, ucolors.len() as u8);
                                        ucolors.push(c);
                                    } else {
                                        fits_16 = false;
                                        break;
                                    }
                                }
                            }

                            if fits_16 {
                                // Mode 2: 4bpp nibble packed tile
                                self.delta_payload.push(2);
                                self.delta_payload.push(ucolors.len() as u8);
                                for c in &ucolors {
                                    self.delta_payload.extend_from_slice(&c.to_le_bytes());
                                }
                                for i in 0..32 {
                                    let c0 = cmap[&tile[i * 2]];
                                    let c1 = cmap[&tile[i * 2 + 1]];
                                    self.delta_payload.push((c0 & 0x0F) | ((c1 & 0x0F) << 4));
                                }
                            } else {
                                // Mode 3: Raw 64 RGB555 pixels
                                self.delta_payload.push(3);
                                for &p in &tile {
                                    self.delta_payload.extend_from_slice(&p.to_le_bytes());
                                }
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

        // Full Keyframe Palette Encoding (resets dictionary)
        self.tile_dict.clear();
        self.cache_head = 0;
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

pub struct TileCacheDecoder {
    pub cache: Box<[[u16; BLOCK_PIXELS]; DICT_CACHE_SIZE]>,
    pub cache_head: u16,
    pub screen: Vec<u16>,
}

impl TileCacheDecoder {
    pub fn new() -> Self {
        Self {
            cache: Box::new([[0u16; BLOCK_PIXELS]; DICT_CACHE_SIZE]),
            cache_head: 0,
            screen: vec![0u16; TOTAL_PIXELS],
        }
    }

    pub fn reset_cache(&mut self) {
        self.cache_head = 0;
    }

    pub fn decode(&mut self, flag: u8, video_payload: &[u8]) -> Result<&[u16], String> {
        let decomp = lz4_flex::decompress_size_prepended(video_payload)
            .map_err(|e| format!("LZ4: {}", e))?;

        match flag {
            4 => {
                self.reset_cache();
                if decomp.len() < 2 { return Err("Short 4bpp".into()); }
                let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                if decomp.len() < 2 + pal_len * 2 + TOTAL_PIXELS / 2 { return Err("Short 4bpp payload".into()); }
                let pal = unsafe {
                    std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
                };
                let packed = &decomp[2 + pal_len * 2..];
                for i in 0..TOTAL_PIXELS / 2 {
                    let b = packed[i];
                    self.screen[i * 2] = pal[(b & 0x0F) as usize];
                    self.screen[i * 2 + 1] = pal[((b >> 4) & 0x0F) as usize];
                }
                Ok(&self.screen)
            }
            2 => {
                self.reset_cache();
                if decomp.len() < 2 { return Err("Short 8bpp".into()); }
                let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                if decomp.len() < 2 + pal_len * 2 + TOTAL_PIXELS { return Err("Short 8bpp payload".into()); }
                let pal = unsafe {
                    std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
                };
                let indices = &decomp[2 + pal_len * 2..];
                for p in 0..TOTAL_PIXELS {
                    self.screen[p] = pal[indices[p] as usize];
                }
                Ok(&self.screen)
            }
            1 => {
                self.reset_cache();
                if decomp.len() != TOTAL_PIXELS * 2 { return Err("Raw frame size mismatch".into()); }
                let raw_pixels = unsafe {
                    std::slice::from_raw_parts(decomp.as_ptr() as *const u16, TOTAL_PIXELS)
                };
                self.screen.copy_from_slice(raw_pixels);
                Ok(&self.screen)
            }
            8 => {
                if decomp.len() < 2 { return Err("Short delta header".into()); }
                let num_blocks = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                let mut offset = 2;

                for _ in 0..num_blocks {
                    if offset + 3 > decomp.len() { return Err("Truncated block header".into()); }
                    let b_idx = u16::from_le_bytes(decomp[offset..offset + 2].try_into().unwrap()) as usize;
                    let mode = decomp[offset + 2];
                    offset += 3;

                    let bx = (b_idx % BLOCKS_X) * BLOCK_SIZE;
                    let by = (b_idx / BLOCKS_X) * BLOCK_SIZE;

                    let mut tile = [0u16; BLOCK_PIXELS];

                    match mode {
                        0 => {
                            // Mode 0: Solid
                            if offset + 2 > decomp.len() { return Err("Truncated solid block".into()); }
                            let c = u16::from_le_bytes(decomp[offset..offset + 2].try_into().unwrap());
                            offset += 2;
                            tile.fill(c);
                        }
                        1 => {
                            // Mode 1: Cache Hit
                            if offset + 2 > decomp.len() { return Err("Truncated cache hit".into()); }
                            let cache_id = u16::from_le_bytes(decomp[offset..offset + 2].try_into().unwrap()) as usize;
                            offset += 2;
                            if cache_id >= DICT_CACHE_SIZE { return Err("Cache ID out of bounds".into()); }
                            tile = self.cache[cache_id];
                        }
                        2 => {
                            // Mode 2: 4bpp tile miss
                            if offset + 1 > decomp.len() { return Err("Truncated 4bpp pal_len".into()); }
                            let pal_len = decomp[offset] as usize;
                            offset += 1;
                            if offset + pal_len * 2 + 32 > decomp.len() { return Err("Truncated 4bpp tile".into()); }
                            let pal = unsafe {
                                std::slice::from_raw_parts(decomp[offset..offset + pal_len * 2].as_ptr() as *const u16, pal_len)
                            };
                            offset += pal_len * 2;
                            let packed = &decomp[offset..offset + 32];
                            offset += 32;

                            for i in 0..32 {
                                let b = packed[i];
                                let c0 = (b & 0x0F) as usize;
                                let c1 = ((b >> 4) & 0x0F) as usize;
                                tile[i * 2] = if c0 < pal_len { pal[c0] } else { 0 };
                                tile[i * 2 + 1] = if c1 < pal_len { pal[c1] } else { 0 };
                            }

                            let head = self.cache_head as usize;
                            self.cache[head] = tile;
                            self.cache_head = (self.cache_head + 1) % (DICT_CACHE_SIZE as u16);
                        }
                        3 => {
                            // Mode 3: Raw tile miss
                            if offset + BLOCK_PIXELS * 2 > decomp.len() { return Err("Truncated raw block".into()); }
                            let raw = unsafe {
                                std::slice::from_raw_parts(decomp[offset..offset + BLOCK_PIXELS * 2].as_ptr() as *const u16, BLOCK_PIXELS)
                            };
                            offset += BLOCK_PIXELS * 2;
                            tile.copy_from_slice(raw);

                            let head = self.cache_head as usize;
                            self.cache[head] = tile;
                            self.cache_head = (self.cache_head + 1) % (DICT_CACHE_SIZE as u16);
                        }
                        _ => return Err("Invalid block mode".into()),
                    }

                    for py in 0..BLOCK_SIZE {
                        let y = by + py;
                        for px in 0..BLOCK_SIZE {
                            let x = bx + px;
                            self.screen[y * GBA_WIDTH + x] = tile[py * BLOCK_SIZE + px];
                        }
                    }
                }
                Ok(&self.screen)
            }
            _ => Err("Unknown flag".into()),
        }
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
        let mut decoder = TileCacheDecoder::new();

        let frame1 = vec![0x1234u16; TOTAL_PIXELS];
        let (flag1, comp1) = encoder.encode(&frame1);
        assert_eq!(flag1, 4, "Initial frame is keyframe");
        let dec1 = decoder.decode(flag1, &comp1).unwrap();
        assert_eq!(dec1, frame1.as_slice(), "Keyframe must be bit-exact");

        // Move a sprite in block 15
        let mut frame2 = frame1.clone();
        for py in 0..8 {
            for px in 0..8 {
                let p = py * GBA_WIDTH + (15 * 8 + px);
                frame2[p] = 0x5678u16;
            }
        }

        let (flag2, comp2) = encoder.encode(&frame2);
        assert_eq!(flag2, 8, "Small update must trigger Delta Block encoding");
        let dec2 = decoder.decode(flag2, &comp2).unwrap();
        assert_eq!(dec2, frame2.as_slice(), "Delta Block decoding must be 100% bit-exact");
    }

    #[test]
    fn test_tile_cache_hit_and_solid_roundtrip() {
        let mut encoder = PaletteEncoder::new();
        let mut decoder = TileCacheDecoder::new();

        // Frame 1: Black keyframe
        let frame1 = vec![0x0000u16; TOTAL_PIXELS];
        let (flag1, comp1) = encoder.encode(&frame1);
        let dec1 = decoder.decode(flag1, &comp1).unwrap();
        assert_eq!(dec1, frame1.as_slice());

        // Frame 2: Introduce a solid red block at block 10 (Mode 0: Solid)
        let mut frame2 = frame1.clone();
        for py in 0..8 {
            for px in 0..8 {
                frame2[py * GBA_WIDTH + (10 * 8 + px)] = 0xF800; // Red
            }
        }
        let (flag2, comp2) = encoder.encode(&frame2);
        assert_eq!(flag2, 8);
        let dec2 = decoder.decode(flag2, &comp2).unwrap();
        assert_eq!(dec2, frame2.as_slice());

        // Frame 3: Introduce a multi-color pattern in block 20 (Mode 2: 4bpp Miss -> Populates cache)
        let mut frame3 = frame2.clone();
        for py in 0..8 {
            for px in 0..8 {
                let color = if (px + py) % 2 == 0 { 0x07E0 } else { 0x001F };
                frame3[py * GBA_WIDTH + (20 * 8 + px)] = color;
            }
        }
        let (flag3, comp3) = encoder.encode(&frame3);
        assert_eq!(flag3, 8);
        let dec3 = decoder.decode(flag3, &comp3).unwrap();
        assert_eq!(dec3, frame3.as_slice());

        // Frame 4: Copy that EXACT same pattern to block 25 (Mode 1: Cache Hit!)
        let mut frame4 = frame3.clone();
        for py in 0..8 {
            for px in 0..8 {
                let color = if (px + py) % 2 == 0 { 0x07E0 } else { 0x001F };
                frame4[py * GBA_WIDTH + (25 * 8 + px)] = color;
            }
        }
        let (flag4, comp4) = encoder.encode(&frame4);
        assert_eq!(flag4, 8);

        // Verify that Frame 4's payload is tiny (< 25 bytes decompressed for 1 cache hit block)
        let decomp4 = lz4_flex::decompress_size_prepended(&comp4).unwrap();
        assert!(decomp4.len() <= 10, "Cache Hit block must be <= 10 bytes decompressed, got {}", decomp4.len());

        let dec4 = decoder.decode(flag4, &comp4).unwrap();
        assert_eq!(dec4, frame4.as_slice(), "Cache Hit must be 100% bit-exact");
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
