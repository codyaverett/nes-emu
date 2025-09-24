use bitflags::bitflags;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 240;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PpuCtrl: u8 {
        const NAMETABLE_ADDR = 0b00000011;
        const VRAM_INCREMENT = 0b00000100;
        const SPRITE_PATTERN = 0b00001000;
        const BG_PATTERN = 0b00010000;
        const SPRITE_SIZE = 0b00100000;
        const MASTER_SLAVE = 0b01000000;
        const NMI_ENABLE = 0b10000000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PpuMask: u8 {
        const GRAYSCALE = 0b00000001;
        const SHOW_BG_LEFT = 0b00000010;
        const SHOW_SPRITES_LEFT = 0b00000100;
        const SHOW_BG = 0b00001000;
        const SHOW_SPRITES = 0b00010000;
        const EMPHASIZE_RED = 0b00100000;
        const EMPHASIZE_GREEN = 0b01000000;
        const EMPHASIZE_BLUE = 0b10000000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PpuStatus: u8 {
        const SPRITE_OVERFLOW = 0b00100000;
        const SPRITE_ZERO_HIT = 0b01000000;
        const VBLANK_STARTED = 0b10000000;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct _Sprite {
    pub y: u8,
    pub tile_id: u8,
    pub attributes: u8,
    pub x: u8,
}

impl _Sprite {
    pub fn _new() -> Self {
        _Sprite {
            y: 0xFF,
            tile_id: 0,
            attributes: 0,
            x: 0,
        }
    }
}

pub struct Ppu {
    pub ctrl: PpuCtrl,
    pub mask: PpuMask,
    pub status: PpuStatus,
    pub oam_addr: u8,
    pub oam_data: [u8; 256],
    pub ppu_data_buffer: u8,
    pub vram: [u8; 0x4000],
    pub palette: [u8; 32],
    
    pub scanline: u16,
    pub cycle: u16,
    pub frame: u64,
    
    pub frame_buffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT * 3],
    pub nmi_interrupt: bool,
    
    // PPU internal registers for scrolling
    v: u16,     // Current VRAM address (15 bits)
    t: u16,     // Temporary VRAM address (15 bits)
    x: u8,      // Fine X scroll (3 bits)
    w: bool,    // Write latch
    
    // Sprite evaluation data
    secondary_oam: [u8; 32],
    sprite_count: u8,
    sprite_zero_in_secondary: bool,
    sprite_patterns: [(u8, u8); 8],
    sprite_positions: [u8; 8],
    sprite_priorities: [u8; 8],
    sprite_indexes: [u8; 8],
}

impl Ppu {
    pub fn new() -> Self {
        let mut ppu = Ppu {
            ctrl: PpuCtrl::empty(),
            mask: PpuMask::empty(),
            status: PpuStatus::empty(),
            oam_addr: 0,
            oam_data: [0; 256],
            ppu_data_buffer: 0,
            vram: [0; 0x4000],
            palette: [0; 32],
            scanline: 0,
            cycle: 0,
            frame: 0,
            frame_buffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT * 3],
            nmi_interrupt: false,
            v: 0,
            t: 0,
            x: 0,
            w: false,
            secondary_oam: [0xFF; 32],
            sprite_count: 0,
            sprite_zero_in_secondary: false,
            sprite_patterns: [(0, 0); 8],
            sprite_positions: [0; 8],
            sprite_priorities: [0; 8],
            sprite_indexes: [0; 8],
        };
        
        // Initialize with default NES palette values
        // Common default palette that shows something
        ppu.palette[0] = 0x22;  // Light blue background (common for SMB)
        
        // Background palettes - set to reasonable defaults
        for i in 0..32 {
            ppu.palette[i] = 0x0F; // Default to black
        }
        ppu.palette[0] = 0x22; // Light blue for universal background
        
        ppu
    }

    pub fn reset(&mut self) {
        self.ctrl = PpuCtrl::empty();
        self.mask = PpuMask::empty();
        self.status = PpuStatus::empty();
        self.oam_addr = 0;
        self.ppu_data_buffer = 0;
        self.scanline = 0;
        self.cycle = 0;
        self.v = 0;
        self.t = 0;
        self.x = 0;
        self.w = false;
        self.nmi_interrupt = false;
    }

    pub fn read_register(&mut self, address: u16) -> u8 {
        match address {
            0x2002 => self.read_status(),
            0x2004 => self.read_oam_data(),
            0x2007 => self.read_ppu_data(),
            _ => 0,
        }
    }

    pub fn write_register(&mut self, address: u16, value: u8) {
        match address {
            0x2000 => self.write_ctrl(value),
            0x2001 => self.write_mask(value),
            0x2003 => self.write_oam_addr(value),
            0x2004 => self.write_oam_data(value),
            0x2005 => self.write_scroll(value),
            0x2006 => self.write_ppu_addr(value),
            0x2007 => self.write_ppu_data(value),
            _ => {}
        }
    }

    fn read_status(&mut self) -> u8 {
        let result = self.status.bits();
        self.status.remove(PpuStatus::VBLANK_STARTED);
        self.w = false;  // Clear write latch
        result
    }

    fn read_oam_data(&self) -> u8 {
        self.oam_data[self.oam_addr as usize]
    }

    fn read_ppu_data(&mut self) -> u8 {
        let addr = self.v & 0x3FFF;
        let result = if addr < 0x3F00 {
            let buffered = self.ppu_data_buffer;
            self.ppu_data_buffer = self.read_vram(addr);
            buffered
        } else {
            self.ppu_data_buffer = self.read_vram(addr - 0x1000);
            self.read_vram(addr)
        };
        
        if self.ctrl.contains(PpuCtrl::VRAM_INCREMENT) {
            self.v = (self.v + 32) & 0x7FFF;
        } else {
            self.v = (self.v + 1) & 0x7FFF;
        }
        result
    }

    fn write_ctrl(&mut self, value: u8) {
        let prev_nmi = self.ctrl.contains(PpuCtrl::NMI_ENABLE);
        self.ctrl = PpuCtrl::from_bits_truncate(value);
        
        if !prev_nmi && self.ctrl.contains(PpuCtrl::NMI_ENABLE) 
            && self.status.contains(PpuStatus::VBLANK_STARTED) {
            self.nmi_interrupt = true;
        }
        
        // Set nametable bits in temporary address
        self.t = (self.t & !0x0C00) | ((value as u16 & 0x03) << 10);
    }

    fn write_mask(&mut self, value: u8) {
        self.mask = PpuMask::from_bits_truncate(value);
    }

    fn write_oam_addr(&mut self, value: u8) {
        self.oam_addr = value;
    }

    fn write_oam_data(&mut self, value: u8) {
        self.oam_data[self.oam_addr as usize] = value;
        self.oam_addr = self.oam_addr.wrapping_add(1);
    }

    fn write_scroll(&mut self, value: u8) {
        if !self.w {
            // First write (X scroll)
            self.x = value & 0x07;  // Fine X scroll (3 bits)
            self.t = (self.t & !0x001F) | ((value as u16) >> 3);  // Coarse X
        } else {
            // Second write (Y scroll)
            self.t = (self.t & !0x73E0) | 
                     (((value as u16) & 0x07) << 12) |  // Fine Y
                     (((value as u16) & 0xF8) << 2);    // Coarse Y
        }
        self.w = !self.w;
    }

    fn write_ppu_addr(&mut self, value: u8) {
        if !self.w {
            // First write (high byte)
            self.t = (self.t & 0x00FF) | ((value as u16 & 0x3F) << 8);
        } else {
            // Second write (low byte)
            self.t = (self.t & 0xFF00) | value as u16;
            self.v = self.t;  // Copy t to v
        }
        self.w = !self.w;
    }

    fn write_ppu_data(&mut self, value: u8) {
        let addr = self.v & 0x3FFF;
        self.write_vram(addr, value);
        
        if self.ctrl.contains(PpuCtrl::VRAM_INCREMENT) {
            self.v = (self.v + 32) & 0x7FFF;
        } else {
            self.v = (self.v + 1) & 0x7FFF;
        }
    }

    pub fn _oam_dma(&mut self, data: &[u8; 256]) {
        self.oam_data.copy_from_slice(data);
    }

    fn read_vram(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.vram[addr as usize],
            0x2000..=0x2FFF => self.vram[(addr & 0x0FFF) as usize + 0x2000],
            0x3000..=0x3EFF => self.vram[((addr - 0x1000) & 0x0FFF) as usize + 0x2000],
            0x3F00..=0x3F1F => {
                let palette_addr = (addr & 0x1F) as usize;
                if palette_addr % 4 == 0 && palette_addr >= 16 {
                    self.palette[palette_addr - 16]
                } else {
                    self.palette[palette_addr]
                }
            }
            0x3F20..=0x3FFF => self.read_vram(0x3F00 | (addr & 0x1F)),
            _ => 0,
        }
    }

    fn write_vram(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.vram[addr as usize] = value,
            0x2000..=0x2FFF => self.vram[(addr & 0x0FFF) as usize + 0x2000] = value,
            0x3000..=0x3EFF => self.vram[((addr - 0x1000) & 0x0FFF) as usize + 0x2000] = value,
            0x3F00..=0x3F1F => {
                let palette_addr = (addr & 0x1F) as usize;
                if palette_addr % 4 == 0 && palette_addr >= 16 {
                    self.palette[palette_addr - 16] = value;
                } else {
                    self.palette[palette_addr] = value;
                }
            }
            0x3F20..=0x3FFF => self.write_vram(0x3F00 | (addr & 0x1F), value),
            _ => {}
        }
    }

    pub fn step(&mut self) {
        self.cycle += 1;
        
        let rendering_enabled = self.mask.contains(PpuMask::SHOW_BG) || self.mask.contains(PpuMask::SHOW_SPRITES);

        if self.scanline < 240 {
            // Visible scanlines (0-239)
            if self.cycle == 1 {
                // Clear sprite data for this scanline
                self.sprite_count = 0;
                self.sprite_zero_in_secondary = false;
            }
            
            // Sprite evaluation for next scanline
            if self.cycle == 257 && self.mask.contains(PpuMask::SHOW_SPRITES) {
                self.evaluate_sprites();
            }
            
            // Sprite fetching
            if self.cycle >= 257 && self.cycle <= 320 && self.mask.contains(PpuMask::SHOW_SPRITES) {
                self.fetch_sprites();
            }
            
            // Render when background or sprites are enabled
            if rendering_enabled && self.cycle >= 1 && self.cycle <= 256 {
                self.render_pixel();
            }
            
            // Scrolling updates for rendering
            if rendering_enabled {
                if self.cycle == 256 {
                    self.increment_y();  // Increment Y at end of visible part
                }
                if self.cycle == 257 {
                    self.copy_x();  // Copy horizontal bits from t to v
                }
            }
        } else if self.scanline == 241 && self.cycle == 1 {
            self.status.insert(PpuStatus::VBLANK_STARTED);
            if self.ctrl.contains(PpuCtrl::NMI_ENABLE) {
                self.nmi_interrupt = true;
            }
        } else if self.scanline == 261 {
            if self.cycle == 1 {
                self.status.remove(PpuStatus::VBLANK_STARTED);
                self.status.remove(PpuStatus::SPRITE_ZERO_HIT);
                self.status.remove(PpuStatus::SPRITE_OVERFLOW);
            }
            
            // Pre-render scanline updates
            if rendering_enabled {
                if self.cycle >= 280 && self.cycle <= 304 {
                    self.copy_y();  // Copy vertical bits from t to v
                }
                if self.cycle == 256 {
                    self.increment_y();
                }
                if self.cycle == 257 {
                    self.copy_x();
                }
            }
        }

        if self.cycle >= 341 {
            self.cycle = 0;
            self.scanline += 1;
            
            if self.scanline > 261 {
                self.scanline = 0;
                self.frame += 1;
            }
        }
    }

    fn render_pixel(&mut self) {
        let x = (self.cycle - 1) as usize;
        let y = self.scanline as usize;
        
        if x < SCREEN_WIDTH && y < SCREEN_HEIGHT {
            let mut bg_pixel = 0u8;
            let mut bg_palette = 0u8;
            let mut sprite_pixel = 0u8;
            let mut sprite_palette = 0u8;
            let mut sprite_priority = false;
            let mut sprite_zero = false;
            
            // Get background pixel if enabled
            if self.mask.contains(PpuMask::SHOW_BG) {
                if x >= 8 || self.mask.contains(PpuMask::SHOW_BG_LEFT) {
                    let bg_data = self.get_background_pixel(x as u16, y as u16);
                    bg_pixel = bg_data & 0x03;
                    bg_palette = bg_data >> 2;
                }
            }
            
            // Get sprite pixel if enabled
            if self.mask.contains(PpuMask::SHOW_SPRITES) {
                if x >= 8 || self.mask.contains(PpuMask::SHOW_SPRITES_LEFT) {
                    let sprite_data = self.get_sprite_pixel(x as u8);
                    if sprite_data.0 > 0 {
                        sprite_pixel = sprite_data.0 & 0x03;
                        sprite_palette = (sprite_data.0 >> 2) & 0x03;
                        sprite_priority = sprite_data.1;
                        sprite_zero = sprite_data.2;
                    }
                }
            }
            
            // Determine which pixel to display
            let (pixel, _is_sprite) = if bg_pixel == 0 && sprite_pixel == 0 {
                (0, false)
            } else if bg_pixel == 0 && sprite_pixel != 0 {
                (0x10 | (sprite_palette << 2) | sprite_pixel, true)
            } else if bg_pixel != 0 && sprite_pixel == 0 {
                ((bg_palette << 2) | bg_pixel, false)
            } else {
                // Both background and sprite are non-transparent
                // Check sprite priority
                if !sprite_priority {
                    // Sprite in front
                    if sprite_zero && x < 255 {
                        // Sprite 0 hit
                        self.status.insert(PpuStatus::SPRITE_ZERO_HIT);
                    }
                    (0x10 | (sprite_palette << 2) | sprite_pixel, true)
                } else {
                    // Background in front
                    if sprite_zero && x < 255 {
                        // Sprite 0 hit
                        self.status.insert(PpuStatus::SPRITE_ZERO_HIT);
                    }
                    ((bg_palette << 2) | bg_pixel, false)
                }
            };
            
            let color = self.get_color_from_palette(pixel);
            
            let pixel_offset = (y * SCREEN_WIDTH + x) * 3;
            self.frame_buffer[pixel_offset] = color.0;
            self.frame_buffer[pixel_offset + 1] = color.1;
            self.frame_buffer[pixel_offset + 2] = color.2;
        }
    }

    fn increment_x(&mut self) {
        // Increment coarse X
        if (self.v & 0x001F) == 31 {
            self.v &= !0x001F;  // Clear coarse X
            self.v ^= 0x0400;   // Switch horizontal nametable
        } else {
            self.v += 1;
        }
    }
    
    fn increment_y(&mut self) {
        // Increment fine Y
        if (self.v & 0x7000) != 0x7000 {
            self.v += 0x1000;
        } else {
            self.v &= !0x7000;  // Clear fine Y
            let mut y = (self.v & 0x03E0) >> 5;  // Get coarse Y
            if y == 29 {
                y = 0;
                self.v ^= 0x0800;  // Switch vertical nametable
            } else if y == 31 {
                y = 0;  // Wrap around without switching nametable
            } else {
                y += 1;
            }
            self.v = (self.v & !0x03E0) | (y << 5);
        }
    }
    
    fn copy_x(&mut self) {
        // Copy horizontal position from t to v
        self.v = (self.v & !0x041F) | (self.t & 0x041F);
    }
    
    fn copy_y(&mut self) {
        // Copy vertical position from t to v
        self.v = (self.v & !0x7BE0) | (self.t & 0x7BE0);
    }
    
    fn get_background_pixel(&self, x: u16, _y: u16) -> u8 {
        // Use current scroll position from v register
        let addr = self.v;
        
        // Extract scroll components from v register
        let coarse_x = (addr & 0x001F) as u16;
        let coarse_y = ((addr >> 5) & 0x001F) as u16;
        let nametable = ((addr >> 10) & 0x0003) as u16;
        let fine_y = ((addr >> 12) & 0x0007) as u16;
        
        // Calculate tile position - x is the pixel position on screen (0-255)
        // We need to add fine X scroll to get the correct pixel within the current tile
        let fine_x = (x + self.x as u16) & 0x07;
        let tile_offset = ((x + self.x as u16) >> 3) as u16;
        let tile_x = (coarse_x + tile_offset) & 0x1F;
        
        // Read from nametable in VRAM
        let nametable_base = 0x2000 | (nametable << 10);
        let nametable_addr = nametable_base + (coarse_y * 32 + tile_x);
        let tile_id = self.read_vram(nametable_addr) as u16;
        
        // Get pattern from CHR ROM area
        let pattern_base = if self.ctrl.contains(PpuCtrl::BG_PATTERN) { 0x1000 } else { 0x0000 };
        let pattern_addr = pattern_base + tile_id * 16 + fine_y;
        
        let low_byte = self.vram[(pattern_addr & 0x1FFF) as usize];
        let high_byte = self.vram[((pattern_addr + 8) & 0x1FFF) as usize];
        
        let bit = 7 - fine_x;
        let pixel = ((high_byte >> bit) & 1) << 1 | ((low_byte >> bit) & 1);
        
        if pixel == 0 {
            return 0; // Universal background
        }
        
        // Get attribute for palette selection
        let attr_table_x = tile_x / 4;
        let attr_table_y = coarse_y / 4;
        let attr_base = nametable_base + 0x3C0;
        let attr_addr = attr_base + attr_table_y * 8 + attr_table_x;
        let attr_byte = self.read_vram(attr_addr);
        
        let palette_shift = ((coarse_y % 4) / 2) * 4 + ((tile_x % 4) / 2) * 2;
        let palette_index = ((attr_byte >> palette_shift) & 0x03) << 2;
        
        palette_index | pixel
    }

    fn get_color_from_palette(&self, index: u8) -> (u8, u8, u8) {
        let palette_entry = self.palette[(index & 0x1F) as usize] & 0x3F;
        // Return a default gray if palette entry is 0 and it's the background
        if palette_entry == 0 && index == 0 {
            return (0x75, 0x75, 0x75); // Default gray background
        }
        NES_PALETTE[palette_entry as usize]
    }

    pub fn get_frame_buffer(&self) -> &[u8] {
        &self.frame_buffer
    }
    
    fn evaluate_sprites(&mut self) {
        // Clear secondary OAM
        for i in 0..32 {
            self.secondary_oam[i] = 0xFF;
        }
        
        let mut secondary_index = 0;
        self.sprite_count = 0;
        self.sprite_zero_in_secondary = false;
        
        let sprite_height = if self.ctrl.contains(PpuCtrl::SPRITE_SIZE) { 16 } else { 8 };
        let y = self.scanline as i16;
        
        // Evaluate all 64 sprites
        for sprite_index in 0..64 {
            if secondary_index >= 32 {
                // Secondary OAM is full, set overflow flag
                self.status.insert(PpuStatus::SPRITE_OVERFLOW);
                break;
            }
            
            let oam_offset = sprite_index * 4;
            let sprite_y = self.oam_data[oam_offset] as i16;
            
            // Check if sprite is on this scanline
            if y >= sprite_y && y < sprite_y + sprite_height {
                // Copy sprite to secondary OAM
                if secondary_index < 32 {
                    for i in 0..4 {
                        self.secondary_oam[secondary_index + i] = self.oam_data[oam_offset + i];
                    }
                    
                    if sprite_index == 0 {
                        self.sprite_zero_in_secondary = true;
                    }
                    
                    self.sprite_indexes[self.sprite_count as usize] = sprite_index as u8;
                    self.sprite_count += 1;
                    secondary_index += 4;
                }
            }
        }
    }
    
    fn fetch_sprites(&mut self) {
        let sprite_height = if self.ctrl.contains(PpuCtrl::SPRITE_SIZE) { 16 } else { 8 };
        
        for i in 0..self.sprite_count.min(8) {
            let oam_offset = i as usize * 4;
            let y_pos = self.secondary_oam[oam_offset];
            let tile_index = self.secondary_oam[oam_offset + 1];
            let attributes = self.secondary_oam[oam_offset + 2];
            let x_pos = self.secondary_oam[oam_offset + 3];
            
            // Calculate the line of the sprite to fetch
            let sprite_y = self.scanline.wrapping_sub(y_pos as u16).wrapping_sub(1);
            
            // Handle vertical flip
            let actual_y = if (attributes & 0x80) != 0 {
                // Vertically flipped
                (sprite_height - 1) - sprite_y
            } else {
                sprite_y
            };
            
            // Determine pattern table address
            let pattern_addr = if sprite_height == 8 {
                // 8x8 sprites
                let base = if self.ctrl.contains(PpuCtrl::SPRITE_PATTERN) { 0x1000 } else { 0x0000 };
                base + (tile_index as u16 * 16) + (actual_y & 7)
            } else {
                // 8x16 sprites
                let bank = (tile_index & 1) as u16 * 0x1000;
                let tile = (tile_index & 0xFE) as u16;
                let offset = if actual_y >= 8 { actual_y - 8 + 16 } else { actual_y };
                bank + (tile * 16) + offset
            };
            
            // Fetch pattern data
            let low_byte = self.vram[(pattern_addr & 0x1FFF) as usize];
            let high_byte = self.vram[((pattern_addr + 8) & 0x1FFF) as usize];
            
            // Handle horizontal flip
            let (low, high) = if (attributes & 0x40) != 0 {
                // Horizontally flipped
                (reverse_byte(low_byte), reverse_byte(high_byte))
            } else {
                (low_byte, high_byte)
            };
            
            self.sprite_patterns[i as usize] = (low, high);
            self.sprite_positions[i as usize] = x_pos;
            self.sprite_priorities[i as usize] = attributes & 0x20;
        }
    }
    
    fn get_sprite_pixel(&self, x: u8) -> (u8, bool, bool) {
        if self.sprite_count == 0 {
            return (0, false, false);
        }
        
        for i in 0..self.sprite_count.min(8) {
            let sprite_x = self.sprite_positions[i as usize];
            
            if x >= sprite_x && x < sprite_x.wrapping_add(8) {
                let pixel_x = x.wrapping_sub(sprite_x);
                let (low, high) = self.sprite_patterns[i as usize];
                
                let bit = 7 - pixel_x;
                let pixel_value = ((high >> bit) & 1) << 1 | ((low >> bit) & 1);
                
                if pixel_value != 0 {
                    // Get palette from attributes
                    let oam_index = (i as usize * 4 + 2).min(31);
                    let attributes = self.secondary_oam[oam_index];
                    let palette = (attributes & 0x03) + 4; // Sprite palettes are 4-7
                    let priority = (attributes & 0x20) != 0;
                    let is_sprite_zero = i == 0 && self.sprite_zero_in_secondary;
                    
                    return ((palette << 2) | pixel_value, priority, is_sprite_zero);
                }
            }
        }
        
        (0, false, false)
    }
}

fn reverse_byte(byte: u8) -> u8 {
    let mut result = 0u8;
    for i in 0..8 {
        if (byte >> i) & 1 != 0 {
            result |= 1 << (7 - i);
        }
    }
    result
}

const NES_PALETTE: [(u8, u8, u8); 64] = [
    (0x7C, 0x7C, 0x7C), (0x00, 0x00, 0xFC), (0x00, 0x00, 0xBC), (0x44, 0x28, 0xBC),
    (0x8F, 0x00, 0x77), (0xAB, 0x00, 0x13), (0xA7, 0x00, 0x00), (0x7F, 0x0B, 0x00),
    (0x43, 0x2F, 0x00), (0x00, 0x47, 0x00), (0x00, 0x51, 0x00), (0x00, 0x3F, 0x17),
    (0x1B, 0x3F, 0x5F), (0x00, 0x00, 0x00), (0x05, 0x05, 0x05), (0x05, 0x05, 0x05),
    
    (0xBC, 0xBC, 0xBC), (0x00, 0x73, 0xEF), (0x23, 0x3B, 0xEF), (0x83, 0x00, 0xF3),
    (0xBF, 0x00, 0xBF), (0xE7, 0x00, 0x5B), (0xDB, 0x2B, 0x00), (0xCB, 0x4F, 0x0F),
    (0x8B, 0x73, 0x00), (0x00, 0x97, 0x00), (0x00, 0xAB, 0x00), (0x00, 0x93, 0x3B),
    (0x00, 0x83, 0x8B), (0x11, 0x11, 0x11), (0x09, 0x09, 0x09), (0x09, 0x09, 0x09),
    
    (0xFF, 0xFF, 0xFF), (0x3F, 0xBF, 0xFF), (0x5F, 0x97, 0xFF), (0xA7, 0x8B, 0xFD),
    (0xF7, 0x7B, 0xFF), (0xFF, 0x77, 0xB7), (0xFF, 0x77, 0x63), (0xFF, 0x9B, 0x3B),
    (0xF3, 0xBF, 0x3F), (0x83, 0xD3, 0x13), (0x4F, 0xDF, 0x4B), (0x58, 0xF8, 0x98),
    (0x00, 0xEB, 0xDB), (0x66, 0x66, 0x66), (0x0D, 0x0D, 0x0D), (0x0D, 0x0D, 0x0D),
    
    (0xFF, 0xFF, 0xFF), (0xAB, 0xE7, 0xFF), (0xC7, 0xD7, 0xFF), (0xD7, 0xCB, 0xFF),
    (0xFF, 0xC7, 0xFF), (0xFF, 0xC7, 0xDB), (0xFF, 0xBF, 0xB3), (0xFF, 0xDB, 0xAB),
    (0xFF, 0xE7, 0xA3), (0xE3, 0xFF, 0xA3), (0xAB, 0xF3, 0xBF), (0xB3, 0xFF, 0xCF),
    (0x9F, 0xFF, 0xF3), (0xDD, 0xDD, 0xDD), (0x11, 0x11, 0x11), (0x11, 0x11, 0x11),
];