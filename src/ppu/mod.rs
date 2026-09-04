use crate::cartridge::{Mapper, Mirroring};
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
    /// 4 KB of nametable RAM (2 KB CIRAM plus room for four-screen boards).
    /// Pattern tables live in the cartridge and are read through `Mapper`.
    pub nametable_ram: [u8; 0x1000],
    pub palette: [u8; 32],

    pub scanline: u16,
    pub cycle: u16,
    pub frame: u64,

    pub frame_buffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT * 3],
    pub nmi_interrupt: bool,

    // PPU internal registers for scrolling
    v: u16,  // Current VRAM address (15 bits)
    t: u16,  // Temporary VRAM address (15 bits)
    x: u8,   // Fine X scroll (3 bits)
    w: bool, // Write latch

    // Background rendering shift registers
    bg_shift_pattern_lo: u16, // Low pattern shift register
    bg_shift_pattern_hi: u16, // High pattern shift register
    bg_shift_attrib_lo: u16,  // Low attribute shift register (16-bit like pattern shifters)
    bg_shift_attrib_hi: u16,  // High attribute shift register (16-bit like pattern shifters)

    // Next tile latches (loaded into shift registers every 8 cycles)
    bg_next_tile_id: u8,     // Next tile ID from nametable
    bg_next_tile_attrib: u8, // Next tile attribute
    bg_next_tile_lsb: u8,    // Next tile pattern low byte
    bg_next_tile_msb: u8,    // Next tile pattern high byte

    // Sprite evaluation data
    secondary_oam: [u8; 32],
    sprite_count: u8,
    sprite_zero_in_secondary: bool,
    sprite_patterns: [(u8, u8); 8],
    sprite_positions: [u8; 8],
    sprite_priorities: [u8; 8],
    sprite_indexes: [u8; 8],
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
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
            nametable_ram: [0; 0x1000],
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
            bg_shift_pattern_lo: 0,
            bg_shift_pattern_hi: 0,
            bg_shift_attrib_lo: 0,
            bg_shift_attrib_hi: 0,
            bg_next_tile_id: 0,
            bg_next_tile_attrib: 0,
            bg_next_tile_lsb: 0,
            bg_next_tile_msb: 0,
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
        ppu.palette[0] = 0x22; // Light blue background (common for SMB)

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
        self.bg_shift_pattern_lo = 0;
        self.bg_shift_pattern_hi = 0;
        self.bg_shift_attrib_lo = 0;
        self.bg_shift_attrib_hi = 0;
        self.bg_next_tile_id = 0;
        self.bg_next_tile_attrib = 0;
        self.bg_next_tile_lsb = 0;
        self.bg_next_tile_msb = 0;
        self.nmi_interrupt = false;
    }

    pub fn read_register(&mut self, address: u16, mapper: &mut dyn Mapper) -> u8 {
        match address {
            0x2002 => self.read_status(),
            0x2004 => self.read_oam_data(),
            0x2007 => self.read_ppu_data(mapper),
            _ => 0,
        }
    }

    pub fn write_register(&mut self, address: u16, value: u8, mapper: &mut dyn Mapper) {
        match address {
            0x2000 => self.write_ctrl(value),
            0x2001 => self.write_mask(value),
            0x2003 => self.write_oam_addr(value),
            0x2004 => self.write_oam_data(value),
            0x2005 => self.write_scroll(value),
            0x2006 => self.write_ppu_addr(value),
            0x2007 => self.write_ppu_data(value, mapper),
            _ => {}
        }
    }

    fn read_status(&mut self) -> u8 {
        let result = self.status.bits();
        self.status.remove(PpuStatus::VBLANK_STARTED);
        self.w = false; // Clear write latch
        result
    }

    fn read_oam_data(&self) -> u8 {
        self.oam_data[self.oam_addr as usize]
    }

    fn read_ppu_data(&mut self, mapper: &mut dyn Mapper) -> u8 {
        let addr = self.v & 0x3FFF;
        let result = if addr < 0x3F00 {
            let buffered = self.ppu_data_buffer;
            self.ppu_data_buffer = self.read_vram(addr, mapper);
            buffered
        } else {
            self.ppu_data_buffer = self.read_vram(addr - 0x1000, mapper);
            self.read_vram(addr, mapper)
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

        if !prev_nmi
            && self.ctrl.contains(PpuCtrl::NMI_ENABLE)
            && self.status.contains(PpuStatus::VBLANK_STARTED)
        {
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
            self.x = value & 0x07; // Fine X scroll (3 bits)
            self.t = (self.t & !0x001F) | ((value as u16) >> 3); // Coarse X
        } else {
            // Second write (Y scroll)
            // Fine Y goes to bits 12-14, coarse Y to bits 5-9
            self.t = (self.t & !0x73E0)
                | (((value as u16) & 0x07) << 12)
                | (((value as u16) & 0xF8) << 2);
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
            self.v = self.t; // Copy t to v
        }
        self.w = !self.w;
    }

    fn write_ppu_data(&mut self, value: u8, mapper: &mut dyn Mapper) {
        let addr = self.v & 0x3FFF;
        self.write_vram(addr, value, mapper);

        if self.ctrl.contains(PpuCtrl::VRAM_INCREMENT) {
            self.v = (self.v + 32) & 0x7FFF;
        } else {
            self.v = (self.v + 1) & 0x7FFF;
        }
    }

    pub fn _oam_dma(&mut self, data: &[u8; 256]) {
        self.oam_data.copy_from_slice(data);
    }

    fn read_vram(&mut self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        match addr {
            0x0000..=0x1FFF => mapper.ppu_read(addr),
            0x2000..=0x2FFF => {
                let index = self.mirror_nametable_addr(addr, mapper.mirroring());
                self.nametable_ram[index as usize]
            }
            0x3000..=0x3EFF => {
                // Mirror of 0x2000-0x2EFF
                self.read_vram(addr - 0x1000, mapper)
            }
            0x3F00..=0x3F1F => {
                let palette_addr = (addr & 0x1F) as usize;
                if palette_addr.is_multiple_of(4) && palette_addr >= 16 {
                    self.palette[palette_addr - 16]
                } else {
                    self.palette[palette_addr]
                }
            }
            0x3F20..=0x3FFF => self.read_vram(0x3F00 | (addr & 0x1F), mapper),
            _ => 0,
        }
    }

    fn write_vram(&mut self, addr: u16, value: u8, mapper: &mut dyn Mapper) {
        match addr {
            0x0000..=0x1FFF => mapper.ppu_write(addr, value),
            0x2000..=0x2FFF => {
                let index = self.mirror_nametable_addr(addr, mapper.mirroring());
                self.nametable_ram[index as usize] = value;
            }
            0x3000..=0x3EFF => {
                // Mirror of 0x2000-0x2EFF
                self.write_vram(addr - 0x1000, value, mapper);
            }
            0x3F00..=0x3F1F => {
                let palette_addr = (addr & 0x1F) as usize;
                if palette_addr.is_multiple_of(4) && palette_addr >= 16 {
                    self.palette[palette_addr - 16] = value;
                } else {
                    self.palette[palette_addr] = value;
                }
            }
            0x3F20..=0x3FFF => self.write_vram(0x3F00 | (addr & 0x1F), value, mapper),
            _ => {}
        }
    }

    /// Map a nametable address ($2000-$2FFF) to an index into `nametable_ram`
    /// using the mirroring the mapper reports right now.
    fn mirror_nametable_addr(&self, addr: u16, mirroring: Mirroring) -> u16 {
        let table = (addr - 0x2000) / 0x400;
        let offset = (addr - 0x2000) % 0x400;

        let mirrored_table = match mirroring {
            Mirroring::Horizontal => {
                // 0,1 -> 0; 2,3 -> 1
                match table {
                    0 | 1 => 0,
                    2 | 3 => 1,
                    _ => table & 0x03,
                }
            }
            Mirroring::Vertical => {
                // 0,2 -> 0; 1,3 -> 1
                match table {
                    0 | 2 => 0,
                    1 | 3 => 1,
                    _ => table & 0x03,
                }
            }
            Mirroring::FourScreen => {
                // Each table is separate (no mirroring)
                table & 0x03
            }
            Mirroring::SingleScreenLower => {
                // All tables map to table 0
                0
            }
            Mirroring::SingleScreenUpper => {
                // All tables map to table 1
                1
            }
        };

        mirrored_table * 0x400 + offset
    }

    pub fn step(&mut self, mapper: &mut dyn Mapper) {
        self.cycle += 1;

        let rendering_enabled =
            self.mask.contains(PpuMask::SHOW_BG) || self.mask.contains(PpuMask::SHOW_SPRITES);

        if self.scanline < 240 {
            // Visible scanlines (0-239)
            if self.cycle == 1 {
                // Clear secondary OAM for sprite evaluation
                for i in 0..32 {
                    self.secondary_oam[i] = 0xFF;
                }
                self.sprite_count = 0;
                self.sprite_zero_in_secondary = false;
            }

            // Sprite evaluation happens during cycles 65-256
            if self.cycle == 65 && self.mask.contains(PpuMask::SHOW_SPRITES) {
                self.evaluate_sprites();
            }

            // Sprite fetching happens during cycles 257-320
            if self.cycle == 257 && self.mask.contains(PpuMask::SHOW_SPRITES) {
                self.fetch_sprites(mapper);
            }

            // Background rendering with cycle-accurate tile fetching
            if rendering_enabled {
                // Update shift registers every cycle during rendering
                if (self.cycle >= 1 && self.cycle <= 256)
                    || (self.cycle >= 321 && self.cycle <= 336)
                {
                    self.update_shifters();

                    // 8-cycle tile fetch pipeline
                    match (self.cycle - 1) % 8 {
                        0 => {
                            // Load previous tile data into shift registers
                            self.load_background_shifters();
                        }
                        1 => {
                            // Fetch nametable byte (tile ID)
                            self.fetch_nametable_byte(mapper);
                        }
                        3 => {
                            // Fetch attribute byte (palette)
                            self.fetch_attribute_byte(mapper);
                        }
                        5 => {
                            // Fetch pattern table low byte
                            self.fetch_pattern_low(mapper);
                        }
                        7 => {
                            // Fetch pattern table high byte
                            self.fetch_pattern_high(mapper);

                            // Increment coarse X after fetching a complete tile
                            if self.cycle < 256 {
                                self.increment_x();
                            }
                        }
                        _ => {}
                    }
                }

                // Render pixel (output from shift registers)
                if self.cycle >= 1 && self.cycle <= 256 {
                    self.render_pixel();
                }

                // Increment Y at end of visible scanline
                if self.cycle == 256 {
                    self.increment_y();
                }

                // Reset horizontal position at end of scanline
                if self.cycle == 257 {
                    self.copy_x();
                }
            } else {
                // Even when rendering is disabled, still render pixels if the cycle is in range
                // This handles the edge case where rendering gets enabled mid-scanline
                if self.cycle >= 1 && self.cycle <= 256 {
                    self.render_pixel();
                }
            }
        } else if self.scanline == 241 && self.cycle == 1 {
            self.status.insert(PpuStatus::VBLANK_STARTED);
            if self.ctrl.contains(PpuCtrl::NMI_ENABLE) {
                self.nmi_interrupt = true;
            }
        } else if self.scanline == 261 {
            // Pre-render scanline (261) - prepares for next frame
            if self.cycle == 1 {
                self.status.remove(PpuStatus::VBLANK_STARTED);
                self.status.remove(PpuStatus::SPRITE_ZERO_HIT);
                self.status.remove(PpuStatus::SPRITE_OVERFLOW);
            }

            // Pre-render scanline updates
            if rendering_enabled {
                // Run the same tile fetching pipeline as visible scanlines
                // This primes the shift registers for the first scanline
                if (self.cycle >= 1 && self.cycle <= 256)
                    || (self.cycle >= 321 && self.cycle <= 336)
                {
                    self.update_shifters();

                    // 8-cycle tile fetch pipeline
                    match (self.cycle - 1) % 8 {
                        0 => {
                            self.load_background_shifters();
                        }
                        1 => {
                            self.fetch_nametable_byte(mapper);
                        }
                        3 => {
                            self.fetch_attribute_byte(mapper);
                        }
                        5 => {
                            self.fetch_pattern_low(mapper);
                        }
                        7 => {
                            self.fetch_pattern_high(mapper);
                            if self.cycle < 256 {
                                self.increment_x();
                            }
                        }
                        _ => {}
                    }
                }

                // Increment Y at end of scanline
                if self.cycle == 256 {
                    self.increment_y();
                }

                // Reset horizontal position
                if self.cycle == 257 {
                    self.copy_x();
                }

                // Copy Y position during vertical blank period (280-304)
                if self.cycle >= 280 && self.cycle <= 304 {
                    self.copy_y();
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
            if self.mask.contains(PpuMask::SHOW_BG)
                && (x >= 8 || self.mask.contains(PpuMask::SHOW_BG_LEFT))
            {
                let bg_data = self.get_background_pixel(x as u16, y as u16);
                bg_pixel = bg_data & 0x03;
                bg_palette = bg_data >> 2;
            }

            // Get sprite pixel if enabled
            if self.mask.contains(PpuMask::SHOW_SPRITES)
                && (x >= 8 || self.mask.contains(PpuMask::SHOW_SPRITES_LEFT))
            {
                let sprite_data = self.get_sprite_pixel(x as u8);
                if sprite_data.0 > 0 {
                    sprite_pixel = sprite_data.0 & 0x03;
                    sprite_palette = (sprite_data.0 >> 2) & 0x03;
                    sprite_priority = sprite_data.1;
                    sprite_zero = sprite_data.2;
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
            self.v &= !0x001F; // Clear coarse X
            self.v ^= 0x0400; // Switch horizontal nametable
        } else {
            self.v += 1;
        }
    }

    fn increment_y(&mut self) {
        // Increment fine Y
        if (self.v & 0x7000) != 0x7000 {
            self.v += 0x1000;
        } else {
            self.v &= !0x7000; // Clear fine Y
            let mut y = (self.v & 0x03E0) >> 5; // Get coarse Y
            if y == 29 {
                y = 0;
                self.v ^= 0x0800; // Switch vertical nametable
            } else if y == 31 {
                y = 0; // Wrap around without switching nametable
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

    // Tile fetch pipeline functions
    fn fetch_nametable_byte(&mut self, mapper: &mut dyn Mapper) {
        // Fetch tile ID from nametable
        // Nametable address: 0x2000 | (v & 0x0FFF)
        let addr = 0x2000 | (self.v & 0x0FFF);
        self.bg_next_tile_id = self.read_vram(addr, mapper);
    }

    fn fetch_attribute_byte(&mut self, mapper: &mut dyn Mapper) {
        // Fetch attribute byte from attribute table
        // Attribute address: 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07)
        let v = self.v;
        let addr = 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07);
        let attribute = self.read_vram(addr, mapper);

        // Determine which 2 bits of the attribute byte to use
        // based on the 2x2 tile position within the 4x4 tile group
        let coarse_x = v & 0x001F;
        let coarse_y = (v >> 5) & 0x001F;
        let shift = ((coarse_y & 0x02) << 1) | (coarse_x & 0x02);

        // Extract the 2-bit palette value
        self.bg_next_tile_attrib = (attribute >> shift) & 0x03;
    }

    fn fetch_pattern_low(&mut self, mapper: &mut dyn Mapper) {
        // Fetch low pattern byte from pattern table
        // Pattern table address: (ctrl.bg_pattern * 0x1000) + (tile_id * 16) + fine_y
        let pattern_table = if self.ctrl.contains(PpuCtrl::BG_PATTERN) {
            0x1000
        } else {
            0x0000
        };
        let fine_y = (self.v >> 12) & 0x07;
        let addr = pattern_table + (self.bg_next_tile_id as u16 * 16) + fine_y;
        self.bg_next_tile_lsb = self.read_vram(addr, mapper);
    }

    fn fetch_pattern_high(&mut self, mapper: &mut dyn Mapper) {
        // Fetch high pattern byte from pattern table
        // Pattern table address: (ctrl.bg_pattern * 0x1000) + (tile_id * 16) + fine_y + 8
        let pattern_table = if self.ctrl.contains(PpuCtrl::BG_PATTERN) {
            0x1000
        } else {
            0x0000
        };
        let fine_y = (self.v >> 12) & 0x07;
        let addr = pattern_table + (self.bg_next_tile_id as u16 * 16) + fine_y + 8;
        self.bg_next_tile_msb = self.read_vram(addr, mapper);
    }

    // Shifter update functions
    fn update_shifters(&mut self) {
        // Shift pattern registers left by 1 bit every cycle
        // This makes the next pixel available at bit 15
        if self.mask.contains(PpuMask::SHOW_BG) {
            self.bg_shift_pattern_lo <<= 1;
            self.bg_shift_pattern_hi <<= 1;
            self.bg_shift_attrib_lo <<= 1;
            self.bg_shift_attrib_hi <<= 1;
        }
    }

    fn load_background_shifters(&mut self) {
        // Load the next tile data into the shift registers
        // This happens every 8 cycles (at the end of each tile fetch)
        // The low 8 bits of the shift registers are loaded with new data
        self.bg_shift_pattern_lo =
            (self.bg_shift_pattern_lo & 0xFF00) | (self.bg_next_tile_lsb as u16);
        self.bg_shift_pattern_hi =
            (self.bg_shift_pattern_hi & 0xFF00) | (self.bg_next_tile_msb as u16);

        // Load attribute bits into each attribute shifter (16-bit registers)
        // The attribute applies to an entire 8-pixel tile, so we fill the low 8 bits
        // with the same palette value (0x00 or 0xFF) while preserving the high 8 bits
        self.bg_shift_attrib_lo = (self.bg_shift_attrib_lo & 0xFF00)
            | if (self.bg_next_tile_attrib & 0x01) != 0 {
                0xFF
            } else {
                0x00
            };
        self.bg_shift_attrib_hi = (self.bg_shift_attrib_hi & 0xFF00)
            | if (self.bg_next_tile_attrib & 0x02) != 0 {
                0xFF
            } else {
                0x00
            };
    }

    fn get_background_pixel(&self, _x: u16, _y: u16) -> u8 {
        // Read pixel from background shift registers
        // The fine X scroll determines which bit to read (15 - fine_x)
        // Shift registers hold 16 bits: the current tile (bits 15-8) and next tile (bits 7-0)

        // Calculate which bit to read based on fine X scroll
        // Bit 15 is the leftmost pixel, bit 0 is the rightmost
        let bit_mux = 15 - self.x;

        // Extract the pattern bits from the shift registers
        let pixel_lo = ((self.bg_shift_pattern_lo >> bit_mux) & 0x01) as u8;
        let pixel_hi = ((self.bg_shift_pattern_hi >> bit_mux) & 0x01) as u8;
        let pixel = (pixel_hi << 1) | pixel_lo;

        if pixel == 0 {
            return 0; // Transparent
        }

        // Extract the palette bits from the attribute shift registers
        // Attribute shifters work the same way as pattern shifters - read from bit_mux position
        let palette_lo = ((self.bg_shift_attrib_lo >> bit_mux) & 0x01) as u8;
        let palette_hi = ((self.bg_shift_attrib_hi >> bit_mux) & 0x01) as u8;
        let palette = (palette_hi << 1) | palette_lo;

        // Combine palette and pixel to get final color index
        (palette << 2) | pixel
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
        let mut secondary_index = 0;
        self.sprite_count = 0;
        self.sprite_zero_in_secondary = false;

        let sprite_height = if self.ctrl.contains(PpuCtrl::SPRITE_SIZE) {
            16
        } else {
            8
        };
        // Sprites are evaluated for the NEXT scanline
        let y = (self.scanline + 1) as i16;

        // Evaluate all 64 sprites
        for sprite_index in 0..64 {
            if secondary_index >= 32 {
                // Secondary OAM is full, set overflow flag
                self.status.insert(PpuStatus::SPRITE_OVERFLOW);
                break;
            }

            let oam_offset = sprite_index * 4;
            let sprite_y = self.oam_data[oam_offset] as i16 + 1; // Sprites are delayed by one scanline

            // Check if sprite is on the next scanline
            if y >= sprite_y && y < sprite_y + sprite_height {
                // Copy sprite to secondary OAM
                if secondary_index < 32 {
                    for i in 0..4 {
                        self.secondary_oam[secondary_index + i] = self.oam_data[oam_offset + i];
                    }

                    if sprite_index == 0 {
                        self.sprite_zero_in_secondary = true;
                    }

                    if self.sprite_count < 8 {
                        self.sprite_indexes[self.sprite_count as usize] = sprite_index as u8;
                    }
                    self.sprite_count += 1;
                    secondary_index += 4;
                }
            }
        }
    }

    fn fetch_sprites(&mut self, mapper: &mut dyn Mapper) {
        let sprite_height = if self.ctrl.contains(PpuCtrl::SPRITE_SIZE) {
            16
        } else {
            8
        };

        for i in 0..self.sprite_count.min(8) {
            let oam_offset = i as usize * 4;
            let y_pos = self.secondary_oam[oam_offset];
            let tile_index = self.secondary_oam[oam_offset + 1];
            let attributes = self.secondary_oam[oam_offset + 2];
            let x_pos = self.secondary_oam[oam_offset + 3];

            // Calculate the line of the sprite to fetch (sprites fetched are for the next scanline)
            let sprite_y = (self.scanline + 1).wrapping_sub(y_pos as u16 + 1);

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
                let base = if self.ctrl.contains(PpuCtrl::SPRITE_PATTERN) {
                    0x1000
                } else {
                    0x0000
                };
                base + (tile_index as u16 * 16) + (actual_y & 7)
            } else {
                // 8x16 sprites
                let bank = (tile_index & 1) as u16 * 0x1000;
                let tile = (tile_index & 0xFE) as u16;
                let offset = if actual_y >= 8 {
                    actual_y - 8 + 16
                } else {
                    actual_y
                };
                bank + (tile * 16) + offset
            };

            // Fetch pattern data
            let low_byte = mapper.ppu_read(pattern_addr & 0x1FFF);
            let high_byte = mapper.ppu_read((pattern_addr + 8) & 0x1FFF);

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
    (0x7C, 0x7C, 0x7C),
    (0x00, 0x00, 0xFC),
    (0x00, 0x00, 0xBC),
    (0x44, 0x28, 0xBC),
    (0x8F, 0x00, 0x77),
    (0xAB, 0x00, 0x13),
    (0xA7, 0x00, 0x00),
    (0x7F, 0x0B, 0x00),
    (0x43, 0x2F, 0x00),
    (0x00, 0x47, 0x00),
    (0x00, 0x51, 0x00),
    (0x00, 0x3F, 0x17),
    (0x1B, 0x3F, 0x5F),
    (0x00, 0x00, 0x00),
    (0x05, 0x05, 0x05),
    (0x05, 0x05, 0x05),
    (0xBC, 0xBC, 0xBC),
    (0x00, 0x73, 0xEF),
    (0x23, 0x3B, 0xEF),
    (0x83, 0x00, 0xF3),
    (0xBF, 0x00, 0xBF),
    (0xE7, 0x00, 0x5B),
    (0xDB, 0x2B, 0x00),
    (0xCB, 0x4F, 0x0F),
    (0x8B, 0x73, 0x00),
    (0x00, 0x97, 0x00),
    (0x00, 0xAB, 0x00),
    (0x00, 0x93, 0x3B),
    (0x00, 0x83, 0x8B),
    (0x11, 0x11, 0x11),
    (0x09, 0x09, 0x09),
    (0x09, 0x09, 0x09),
    (0xFF, 0xFF, 0xFF),
    (0x3F, 0xBF, 0xFF),
    (0x5F, 0x97, 0xFF),
    (0xA7, 0x8B, 0xFD),
    (0xF7, 0x7B, 0xFF),
    (0xFF, 0x77, 0xB7),
    (0xFF, 0x77, 0x63),
    (0xFF, 0x9B, 0x3B),
    (0xF3, 0xBF, 0x3F),
    (0x83, 0xD3, 0x13),
    (0x4F, 0xDF, 0x4B),
    (0x58, 0xF8, 0x98),
    (0x00, 0xEB, 0xDB),
    (0x66, 0x66, 0x66),
    (0x0D, 0x0D, 0x0D),
    (0x0D, 0x0D, 0x0D),
    (0xFF, 0xFF, 0xFF),
    (0xAB, 0xE7, 0xFF),
    (0xC7, 0xD7, 0xFF),
    (0xD7, 0xCB, 0xFF),
    (0xFF, 0xC7, 0xFF),
    (0xFF, 0xC7, 0xDB),
    (0xFF, 0xBF, 0xB3),
    (0xFF, 0xDB, 0xAB),
    (0xFF, 0xE7, 0xA3),
    (0xE3, 0xFF, 0xA3),
    (0xAB, 0xF3, 0xBF),
    (0xB3, 0xFF, 0xCF),
    (0x9F, 0xFF, 0xF3),
    (0xDD, 0xDD, 0xDD),
    (0x11, 0x11, 0x11),
    (0x11, 0x11, 0x11),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::mapper3::Mapper3;
    use crate::cartridge::mapper4::Mapper4;
    use crate::cartridge::Cartridge;

    fn tagged_chr(banks: usize) -> Vec<u8> {
        let mut chr = Vec::new();
        for bank in 0..banks {
            chr.extend(vec![0x10 + bank as u8; 0x2000]);
        }
        chr
    }

    /// Set the VRAM address through $2006 and read $2007 twice (the first
    /// read only fills the buffer for addresses below the palette).
    fn read_ppu_addr(ppu: &mut Ppu, mapper: &mut dyn Mapper, addr: u16) -> u8 {
        ppu.write_register(0x2006, (addr >> 8) as u8, mapper);
        ppu.write_register(0x2006, addr as u8, mapper);
        ppu.read_register(0x2007, mapper);
        ppu.read_register(0x2007, mapper)
    }

    fn write_ppu_addr(ppu: &mut Ppu, mapper: &mut dyn Mapper, addr: u16, value: u8) {
        ppu.write_register(0x2006, (addr >> 8) as u8, mapper);
        ppu.write_register(0x2006, addr as u8, mapper);
        ppu.write_register(0x2007, value, mapper);
    }

    #[test]
    fn pattern_table_reads_follow_chr_bank_switch() {
        let mut ppu = Ppu::new();
        let mut mapper = Mapper3::new(vec![0; 0x8000], tagged_chr(2), Mirroring::Vertical);

        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0000), 0x10);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x1FFF), 0x10);

        mapper.cpu_write(0x8000, 1);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0000), 0x11);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x1FFF), 0x11);
    }

    #[test]
    fn chr_ram_writes_go_through_the_mapper() {
        let mut ppu = Ppu::new();
        let cart = Cartridge::build_mapper(0, vec![0; 0x8000], vec![], Mirroring::Vertical);
        let mut mapper = cart;

        write_ppu_addr(&mut ppu, mapper.as_mut(), 0x0123, 0xA5);
        assert_eq!(mapper.ppu_read(0x0123), 0xA5);
        assert_eq!(read_ppu_addr(&mut ppu, mapper.as_mut(), 0x0123), 0xA5);
    }

    #[test]
    fn background_fetch_uses_mapper_pattern_bytes() {
        let mut ppu = Ppu::new();
        let mut mapper = Mapper3::new(vec![0; 0x8000], tagged_chr(2), Mirroring::Vertical);
        ppu.bg_next_tile_id = 0x05;
        ppu.v = 0;

        ppu.fetch_pattern_low(&mut mapper);
        ppu.fetch_pattern_high(&mut mapper);
        assert_eq!((ppu.bg_next_tile_lsb, ppu.bg_next_tile_msb), (0x10, 0x10));

        mapper.cpu_write(0x8000, 1);
        ppu.fetch_pattern_low(&mut mapper);
        ppu.fetch_pattern_high(&mut mapper);
        assert_eq!((ppu.bg_next_tile_lsb, ppu.bg_next_tile_msb), (0x11, 0x11));
    }

    #[test]
    fn nametable_mirroring_is_queried_per_access() {
        let mut ppu = Ppu::new();
        let mut mapper = Mapper4::new(vec![0; 0x8000], tagged_chr(1), Mirroring::Vertical);

        // Vertical: $2000 and $2800 share a table; $2400 is separate
        write_ppu_addr(&mut ppu, &mut mapper, 0x2000, 0x11);
        write_ppu_addr(&mut ppu, &mut mapper, 0x2400, 0x22);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x2800), 0x11);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x2C00), 0x22);

        // Switch the MMC3 to horizontal: $2000 and $2400 now share a table
        mapper.cpu_write(0xA000, 1);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x2400), 0x11);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x2800), 0x22);
    }

    #[test]
    fn mirror_nametable_addr_modes() {
        let ppu = Ppu::new();
        let a = |m: Mirroring, addr: u16| ppu.mirror_nametable_addr(addr, m);
        assert_eq!(a(Mirroring::Horizontal, 0x2400), 0x000);
        assert_eq!(a(Mirroring::Horizontal, 0x2800), 0x400);
        assert_eq!(a(Mirroring::Vertical, 0x2400), 0x400);
        assert_eq!(a(Mirroring::Vertical, 0x2800), 0x000);
        assert_eq!(a(Mirroring::SingleScreenLower, 0x2C05), 0x005);
        assert_eq!(a(Mirroring::SingleScreenUpper, 0x2005), 0x405);
        assert_eq!(a(Mirroring::FourScreen, 0x2C00), 0xC00);
    }
}
