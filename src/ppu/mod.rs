use crate::cartridge::{Mapper, Mirroring};
use bitflags::bitflags;

/// PPU dots A12 must stay low before the next rising edge counts. Hardware
/// wants "three falling edges of M2" (about nine dots); the shortest low run
/// the rendering pipeline produces that must NOT count is the nine dots from
/// the dummy nametable fetch at 337 to the first pattern fetch of the next
/// line at dot 4 (mmc3_test_2 4, tests 10-13), while the 64-dot sprite
/// interval and vblank must. See docs/debugging/PPU_SPRITE_PIPELINE.md.
const A12_FILTER_CYCLES: u16 = 10;

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

/// Frames a latched I/O bus bit survives without being refreshed before it
/// decays to 0. Hardware takes roughly 600 ms (36 NTSC frames); blargg's
/// ppu_open_bus expects the value intact immediately after a write and gone
/// after about a second.
pub const IO_BUS_DECAY_FRAMES: u64 = 36;

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

    /// State of PPU address line A12 the last time the address bus was
    /// driven, plus how many PPU cycles it has been low. Together they
    /// implement the MMC3 A12 edge filter (see `drive_addr_bus`).
    a12_last: bool,
    a12_low_cycles: u16,
    pub frame: u64,

    /// PPU I/O bus latch ("decay register"). Every CPU write to $2000-$2007
    /// loads all eight bits; every CPU read loads the bits the register
    /// drives and returns the latch for the rest. Bits that are not
    /// refreshed decay to 0 after `IO_BUS_DECAY_FRAMES` (see `io_bus()`).
    io_bus: u8,
    /// PPU frame in which each latch bit was last refreshed.
    io_bus_stamp: [u64; 8],

    pub frame_buffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT * 3],
    /// Rising edge of the NMI output line, held until the CPU samples it.
    /// The line itself is `VBLANK_STARTED && NMI_ENABLE`; if the line drops
    /// again before the CPU has sampled the edge (a `$2002` read or an NMI
    /// disable in the same CPU cycle) the edge is withdrawn, which is what
    /// makes NMI suppression work.
    pub nmi_interrupt: bool,
    nmi_line: bool,
    /// Set by a `$2002` read on the dot just before the vblank flag would be
    /// set: that frame's flag (and therefore its NMI) never appears.
    suppress_vbl: bool,

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

    // Sprite evaluation unit (cycles 65-256, evaluates the NEXT line).
    // See docs/debugging/PPU_SPRITE_PIPELINE.md.
    secondary_oam: [u8; 32],
    /// Byte read from OAM on the last odd cycle; what `$2004` returns
    /// while the evaluation unit owns the OAM bus.
    oam_copy_buffer: u8,
    /// Sprite index (n) and byte within the sprite (m) the evaluation unit
    /// is looking at. OAMADDR is rewritten from these after every write
    /// cycle so `$2004` reads track the evaluation.
    eval_n: u8,
    eval_m: u8,
    /// Write pointer into secondary OAM (0-32; 32 means full).
    eval_sec_addr: u8,
    /// The sprite being examined was found in range (copy in progress).
    eval_in_range: bool,
    /// Evaluation has walked off the end of OAM (or finished the overflow
    /// sprite); the remaining cycles only increment n.
    eval_done: bool,
    /// Reads left in the overflow sprite once the flag has been set.
    eval_overflow_reads: u8,
    /// The entry examined first this line is still the one being copied.
    eval_first: bool,
    /// The first entry examined (sprite 0) was copied to secondary OAM.
    sprite_zero_next: bool,

    // Sprite render units, loaded during cycles 257-320 for the next line.
    sprite_count: u8,
    sprite_zero_in_units: bool,
    sprite_patterns: [(u8, u8); 8],
    sprite_positions: [u8; 8],
    sprite_attributes: [u8; 8],
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
            a12_last: false,
            a12_low_cycles: 0,
            frame: 0,
            io_bus: 0,
            io_bus_stamp: [0; 8],
            frame_buffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT * 3],
            nmi_interrupt: false,
            nmi_line: false,
            suppress_vbl: false,
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
            oam_copy_buffer: 0xFF,
            eval_n: 0,
            eval_m: 0,
            eval_sec_addr: 0,
            eval_in_range: false,
            eval_done: false,
            eval_overflow_reads: 0,
            eval_first: false,
            sprite_zero_next: false,
            sprite_count: 0,
            sprite_zero_in_units: false,
            sprite_patterns: [(0, 0); 8],
            sprite_positions: [0; 8],
            sprite_attributes: [0; 8],
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
        self.a12_last = false;
        self.a12_low_cycles = 0;
        self.io_bus = 0;
        self.io_bus_stamp = [0; 8];
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
        self.nmi_line = false;
        self.suppress_vbl = false;
    }

    pub fn read_register(&mut self, address: u16, mapper: &mut dyn Mapper) -> u8 {
        match address {
            0x2002 => {
                // Only the three flag bits are driven; bits 4-0 float on
                // the I/O bus and the read leaves them untouched.
                let flags = self.read_status() & 0xE0;
                self.refresh_io_bus(flags, 0xE0);
                flags | (self.io_bus() & 0x1F)
            }
            0x2004 => {
                let value = self.read_oam_data();
                self.refresh_io_bus(value, 0xFF);
                value
            }
            0x2007 => {
                let palette = (self.v & 0x3FFF) >= 0x3F00;
                let value = self.read_ppu_data(mapper);
                if palette {
                    // Palette entries are 6 bits wide; bits 7-6 come from
                    // the latch and are not refreshed by the read.
                    let value = value & 0x3F;
                    self.refresh_io_bus(value, 0x3F);
                    value | (self.io_bus() & 0xC0)
                } else {
                    self.refresh_io_bus(value, 0xFF);
                    value
                }
            }
            // Write-only registers: nothing drives the bus, so the CPU
            // sees whatever was last left on it.
            _ => self.io_bus(),
        }
    }

    pub fn write_register(&mut self, address: u16, value: u8, mapper: &mut dyn Mapper) {
        // Every write, including to the read-only $2002, loads the latch.
        self.refresh_io_bus(value, 0xFF);
        match address {
            0x2000 => self.write_ctrl(value),
            0x2001 => self.write_mask(value),
            0x2003 => self.write_oam_addr(value),
            0x2004 => self.write_oam_data(value),
            0x2005 => self.write_scroll(value),
            0x2006 => self.write_ppu_addr(value, mapper),
            0x2007 => self.write_ppu_data(value, mapper),
            _ => {}
        }
    }

    /// Load the bits of `value` selected by `mask` into the I/O bus latch
    /// and restart their decay timers.
    fn refresh_io_bus(&mut self, value: u8, mask: u8) {
        self.io_bus = (self.io_bus & !mask) | (value & mask);
        let frame = self.frame;
        for (bit, stamp) in self.io_bus_stamp.iter_mut().enumerate() {
            if mask & (1 << bit) != 0 {
                *stamp = frame;
            }
        }
    }

    /// Current I/O bus value with decayed bits cleared. Decay is evaluated
    /// lazily here rather than every dot; only set bits can decay, so a
    /// zero latch costs nothing.
    fn io_bus(&mut self) -> u8 {
        if self.io_bus != 0 {
            let frame = self.frame;
            for (bit, stamp) in self.io_bus_stamp.iter().enumerate() {
                if frame.saturating_sub(*stamp) > IO_BUS_DECAY_FRAMES {
                    self.io_bus &= !(1 << bit);
                }
            }
        }
        self.io_bus
    }

    fn read_status(&mut self) -> u8 {
        // Reading one dot before the flag is set returns it clear and stops
        // it from being set at all this frame. Reading on the dot it is set
        // or the one after returns it set, and the read clears it before the
        // CPU samples the NMI line, so no NMI occurs (`update_nmi_line`).
        if self.scanline == 241 && self.cycle == 0 {
            self.suppress_vbl = true;
        }
        let result = self.status.bits();
        self.status.remove(PpuStatus::VBLANK_STARTED);
        self.w = false; // Clear write latch
        self.update_nmi_line();
        result
    }

    /// Recompute the NMI output line and latch a rising edge for the CPU.
    /// A falling edge withdraws an edge the CPU has not sampled yet.
    fn update_nmi_line(&mut self) {
        let line = self.status.contains(PpuStatus::VBLANK_STARTED)
            && self.ctrl.contains(PpuCtrl::NMI_ENABLE);
        if line && !self.nmi_line {
            self.nmi_interrupt = true;
        } else if !line {
            self.nmi_interrupt = false;
        }
        self.nmi_line = line;
    }

    /// $2004 read. Never modifies OAMADDR.
    ///
    /// While rendering is active the PPU's sprite evaluation unit owns the
    /// OAM bus, so the CPU sees whatever byte it is examining: $FF during
    /// the secondary OAM clear (cycles 1-64), the byte last read from OAM
    /// during evaluation (65-256), and the secondary OAM byte the sprite
    /// fetch is using (257-320: Y, tile, attribute, then X for the rest of
    /// the slot). Outside rendering the read returns `oam_data[oam_addr]`.
    fn read_oam_data(&self) -> u8 {
        if !self.is_rendering() {
            return self.oam_data[self.oam_addr as usize];
        }
        match self.cycle {
            1..=64 => 0xFF,
            257..=320 => {
                let off = (self.cycle - 257) as usize;
                self.secondary_oam[(off / 8) * 4 + (off % 8).min(3)]
            }
            _ => self.oam_copy_buffer,
        }
    }

    fn sprite_height(&self) -> u8 {
        if self.ctrl.contains(PpuCtrl::SPRITE_SIZE) {
            16
        } else {
            8
        }
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
        self.drive_addr_bus(self.v, mapper);
        result
    }

    fn write_ctrl(&mut self, value: u8) {
        self.ctrl = PpuCtrl::from_bits_truncate(value);
        // Enabling NMI while the flag is set raises the line immediately;
        // disabling it drops the line and withdraws an unsampled edge.
        self.update_nmi_line();

        // Set nametable bits in temporary address
        self.t = (self.t & !0x0C00) | ((value as u16 & 0x03) << 10);
    }

    fn write_mask(&mut self, value: u8) {
        self.mask = PpuMask::from_bits_truncate(value);
    }

    /// $2003 write. The OAMADDR corruption that a write during rendering
    /// causes on hardware is intentionally not modelled here.
    fn write_oam_addr(&mut self, value: u8) {
        self.oam_addr = value;
    }

    /// $2004 write.
    ///
    /// Outside rendering the byte is stored at OAMADDR and OAMADDR advances by
    /// one. Bits 2-4 of every sprite's attribute byte (offset 2 of each
    /// 4-byte entry) do not physically exist, so they are dropped on write and
    /// always read back as 0. OAM DMA funnels through this path too.
    ///
    /// During rendering the data is ignored, but OAMADDR still performs the
    /// glitchy increment documented on nesdev: bits 2-7 advance by one sprite
    /// (4 bytes) while bits 0-1 are left untouched.
    fn write_oam_data(&mut self, value: u8) {
        if self.is_rendering() {
            let high = (self.oam_addr & 0xFC).wrapping_add(4);
            self.oam_addr = high | (self.oam_addr & 0x03);
            return;
        }
        self.oam_data[self.oam_addr as usize] = Self::mask_oam_byte(self.oam_addr, value);
        self.oam_addr = self.oam_addr.wrapping_add(1);
    }

    /// Drop the unimplemented bits 2-4 of attribute bytes; other bytes pass
    /// through unchanged.
    fn mask_oam_byte(addr: u8, value: u8) -> u8 {
        if addr & 0x03 == 2 {
            value & 0xE3
        } else {
            value
        }
    }

    /// True while the PPU is actively rendering: background or sprites are
    /// enabled and the current line is visible or the pre-render line.
    fn is_rendering(&self) -> bool {
        let enabled =
            self.mask.contains(PpuMask::SHOW_BG) || self.mask.contains(PpuMask::SHOW_SPRITES);
        enabled && (self.scanline < 240 || self.scanline == 261)
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

    fn write_ppu_addr(&mut self, value: u8, mapper: &mut dyn Mapper) {
        if !self.w {
            // First write (high byte)
            self.t = (self.t & 0x00FF) | ((value as u16 & 0x3F) << 8);
        } else {
            // Second write (low byte)
            self.t = (self.t & 0xFF00) | value as u16;
            self.v = self.t; // Copy t to v
            self.drive_addr_bus(self.v, mapper);
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
        self.drive_addr_bus(self.v, mapper);
    }

    pub fn _oam_dma(&mut self, data: &[u8; 256]) {
        for (i, (dst, src)) in self.oam_data.iter_mut().zip(data).enumerate() {
            *dst = Self::mask_oam_byte(i as u8, *src);
        }
    }

    /// Record a new value on the PPU address bus and report a filtered A12
    /// rising edge to the mapper. The MMC3 only counts a rise if A12 was low
    /// for a few cycles first, which ignores the rapid toggling between
    /// pattern fetches when background and sprites use different tables.
    fn drive_addr_bus(&mut self, addr: u16, mapper: &mut dyn Mapper) {
        let a12 = addr & 0x1000 != 0;
        if a12 {
            if !self.a12_last && self.a12_low_cycles >= A12_FILTER_CYCLES {
                mapper.ppu_a12_rise();
            }
            self.a12_low_cycles = 0;
        }
        self.a12_last = a12;
    }

    fn read_vram(&mut self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        self.drive_addr_bus(addr, mapper);
        match addr {
            0x0000..=0x1FFF => {
                // Read first, then report the completed fetch: the MMC2
                // latch flips after the byte at $xFD8/$xFE8 is out.
                let value = mapper.ppu_read(addr);
                mapper.ppu_fetch(addr);
                value
            }
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
        self.drive_addr_bus(addr, mapper);
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

        if !self.a12_last {
            self.a12_low_cycles = self.a12_low_cycles.saturating_add(1);
        }

        // Scanline clock for mappers that count scanlines directly (MMC5).
        // MMC3 is clocked from A12 edges instead; see `drive_addr_bus`.
        if rendering_enabled && self.cycle == 260 && (self.scanline < 240 || self.scanline == 261) {
            mapper.clock_scanline();
        }

        if self.scanline < 240 {
            // Visible scanlines (0-239)
            if rendering_enabled {
                self.sprite_pipeline_step(mapper);
            }

            if rendering_enabled {
                self.background_pipeline_step(mapper);

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
            if !self.suppress_vbl {
                self.status.insert(PpuStatus::VBLANK_STARTED);
            }
            self.suppress_vbl = false;
            self.update_nmi_line();
        } else if self.scanline == 261 {
            // Pre-render scanline (261) - prepares for next frame
            if self.cycle == 1 {
                self.status.remove(PpuStatus::VBLANK_STARTED);
                self.status.remove(PpuStatus::SPRITE_ZERO_HIT);
                self.status.remove(PpuStatus::SPRITE_OVERFLOW);
                self.update_nmi_line();
            }

            // Pre-render scanline updates
            if rendering_enabled {
                // The secondary OAM clear and the sprite fetch slots run
                // here too, but no evaluation, so every slot is empty and
                // line 0 never shows sprites. The eight dummy fetches are
                // the 241st MMC3 clock of a frame.
                self.sprite_pipeline_step(mapper);
                // Run the same tile fetching pipeline as visible scanlines
                // This primes the shift registers for the first scanline
                self.background_pipeline_step(mapper);

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

        // Odd frames with rendering enabled are one dot short: the last dot
        // of the pre-render line is skipped.
        if self.scanline == 261 && self.cycle == 338 && self.frame % 2 == 1 && rendering_enabled {
            self.cycle = 339;
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

    /// One dot of the background fetch pipeline on a line with rendering
    /// enabled. Every access takes two dots: the data is read on the second
    /// and the address of the next access is put on the bus at the end of
    /// that same dot, which is why the MMC3 sees the first pattern fetch of
    /// a tile at dots 4, 12, ... (and 324 for the prefetch) rather than 5.
    /// Dots 337-340 perform the two dummy nametable fetches.
    fn background_pipeline_step(&mut self, mapper: &mut dyn Mapper) {
        if (self.cycle >= 1 && self.cycle <= 256) || (self.cycle >= 321 && self.cycle <= 336) {
            self.update_shifters();

            match (self.cycle - 1) % 8 {
                0 => self.load_background_shifters(),
                1 => {
                    self.fetch_nametable_byte(mapper);
                    self.drive_addr_bus(self.bg_fetch_addr(1), mapper);
                }
                3 => {
                    self.fetch_attribute_byte(mapper);
                    self.drive_addr_bus(self.bg_fetch_addr(2), mapper);
                }
                5 => {
                    self.fetch_pattern_low(mapper);
                    self.drive_addr_bus(self.bg_fetch_addr(3), mapper);
                }
                7 => {
                    self.fetch_pattern_high(mapper);
                    // Increment coarse X after every tile, at 256 and at
                    // 328/336 too (that is what puts tile 1 after tile 0
                    // on the next line; skipping it shifts the picture by
                    // 8 pixels), then put the next nametable address on
                    // the bus.
                    self.increment_x();
                    self.drive_addr_bus(self.bg_fetch_addr(0), mapper);
                }
                _ => {}
            }
        } else if self.cycle == 338 || self.cycle == 340 {
            let addr = self.bg_fetch_addr(0);
            self.read_vram(addr, mapper);
        }
    }

    /// Address of background access `phase` for the tile at `v`: 0 the
    /// nametable byte, 1 the attribute byte, 2 and 3 the pattern low and
    /// high bytes (using the tile id latched by the nametable fetch).
    fn bg_fetch_addr(&self, phase: u8) -> u16 {
        let v = self.v;
        match phase {
            0 => 0x2000 | (v & 0x0FFF),
            1 => 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07),
            _ => {
                let pattern_table = if self.ctrl.contains(PpuCtrl::BG_PATTERN) {
                    0x1000
                } else {
                    0x0000
                };
                let fine_y = (v >> 12) & 0x07;
                let plane = if phase == 3 { 8 } else { 0 };
                pattern_table + (self.bg_next_tile_id as u16 * 16) + fine_y + plane
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
                let sprite_data = self.get_sprite_pixel(x as u16);
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

    // Tile fetch pipeline functions (each is the read half of an access;
    // see `background_pipeline_step` for the bus timing)
    fn fetch_nametable_byte(&mut self, mapper: &mut dyn Mapper) {
        let addr = self.bg_fetch_addr(0);
        self.bg_next_tile_id = self.read_vram(addr, mapper);
    }

    fn fetch_attribute_byte(&mut self, mapper: &mut dyn Mapper) {
        let v = self.v;
        let addr = self.bg_fetch_addr(1);
        let attribute = self.read_vram(addr, mapper);

        // Pick the 2 bits for this tile's 2x2 quadrant of the 4x4 group.
        let coarse_x = v & 0x001F;
        let coarse_y = (v >> 5) & 0x001F;
        let shift = ((coarse_y & 0x02) << 1) | (coarse_x & 0x02);
        self.bg_next_tile_attrib = (attribute >> shift) & 0x03;
    }

    fn fetch_pattern_low(&mut self, mapper: &mut dyn Mapper) {
        let addr = self.bg_fetch_addr(2);
        self.bg_next_tile_lsb = self.read_vram(addr, mapper);
    }

    fn fetch_pattern_high(&mut self, mapper: &mut dyn Mapper) {
        let addr = self.bg_fetch_addr(3);
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

    /// One dot of the sprite pipeline on a visible or pre-render line with
    /// rendering enabled. Hardware timing (nesdev "PPU sprite evaluation"):
    ///
    /// * 1-64: secondary OAM is cleared to $FF.
    /// * 65-256: OAM is read on odd dots and secondary OAM written on even
    ///   dots, evaluating the next line's sprites (visible lines only).
    /// * 257-320: eight fetch slots of eight dots each load the render
    ///   units; OAMADDR is held at 0.
    fn sprite_pipeline_step(&mut self, mapper: &mut dyn Mapper) {
        match self.cycle {
            1 => {
                self.secondary_oam = [0xFF; 32];
                self.oam_copy_buffer = 0xFF;
                self.eval_sec_addr = 0;
                self.sprite_zero_next = false;
            }
            65..=256 if self.scanline < 240 => self.evaluate_sprites_cycle(),
            257..=320 => {
                self.oam_addr = 0;
                self.fetch_sprites_cycle(mapper);
            }
            _ => {}
        }
    }

    /// One dot of sprite evaluation (cycles 65-256).
    ///
    /// Odd dots read `OAM[n][m]` into the copy buffer; even dots act on it.
    /// While fewer than eight sprites have been found, an in-range Y copies
    /// the four bytes of the entry into secondary OAM (eight dots per
    /// sprite), otherwise n advances (two dots per sprite). Once secondary
    /// OAM is full the search continues with the hardware bug: a miss
    /// increments both n and m, so the byte compared as Y walks diagonally
    /// through OAM; a hit sets the overflow flag and consumes three more
    /// reads, after which evaluation is done. n and m are written back to
    /// OAMADDR after every even dot, which is why `$2004` reads follow it.
    fn evaluate_sprites_cycle(&mut self) {
        if self.cycle == 65 {
            self.eval_n = self.oam_addr >> 2;
            self.eval_m = self.oam_addr & 0x03;
            self.eval_sec_addr = 0;
            self.eval_in_range = false;
            self.eval_done = false;
            self.eval_overflow_reads = 0;
            self.eval_first = true;
            self.sprite_zero_next = false;
        }

        if self.cycle & 1 == 1 {
            self.oam_copy_buffer = self.oam_data[self.oam_addr as usize];
            return;
        }

        if self.eval_done {
            // Keep walking n; with secondary OAM full the "write" turns
            // into a read of secondary OAM.
            self.eval_n = (self.eval_n + 1) & 0x3F;
            if self.eval_sec_addr >= 32 {
                self.oam_copy_buffer = self.secondary_oam[(self.eval_sec_addr & 0x1F) as usize];
            }
        } else {
            let y = self.oam_copy_buffer as u16;
            let height = self.sprite_height() as u16;
            if !self.eval_in_range && self.scanline >= y && self.scanline < y + height {
                self.eval_in_range = true;
            }

            if self.eval_sec_addr < 32 {
                self.secondary_oam[self.eval_sec_addr as usize] = self.oam_copy_buffer;
                if self.eval_in_range {
                    self.eval_m = self.eval_m.wrapping_add(1);
                    self.eval_sec_addr += 1;
                    if self.eval_first {
                        self.sprite_zero_next = true;
                    }
                    if self.eval_sec_addr & 0x03 == 0 {
                        // All four bytes copied; on to the next entry.
                        self.eval_in_range = false;
                        self.eval_m = 0;
                        self.eval_first = false;
                        self.advance_eval_n();
                    }
                } else {
                    self.eval_first = false;
                    self.advance_eval_n();
                }
            } else {
                self.oam_copy_buffer = self.secondary_oam[(self.eval_sec_addr & 0x1F) as usize];
                if self.eval_in_range {
                    // Ninth sprite found: flag it and read out its
                    // remaining bytes with m carrying into n.
                    self.status.insert(PpuStatus::SPRITE_OVERFLOW);
                    self.eval_m += 1;
                    if self.eval_m == 4 {
                        self.eval_m = 0;
                        self.eval_n = (self.eval_n + 1) & 0x3F;
                    }
                    if self.eval_overflow_reads == 0 {
                        self.eval_overflow_reads = 3;
                    } else {
                        self.eval_overflow_reads -= 1;
                        if self.eval_overflow_reads == 0 {
                            self.eval_done = true;
                            self.eval_m = 0;
                        }
                    }
                } else {
                    // The hardware bug: n and m advance together, without
                    // carry, so the next "Y" comes from a different byte.
                    self.eval_n = (self.eval_n + 1) & 0x3F;
                    self.eval_m = (self.eval_m + 1) & 0x03;
                    if self.eval_n == 0 {
                        self.eval_done = true;
                    }
                }
            }
        }

        self.oam_addr = (self.eval_n << 2) | (self.eval_m & 0x03);
    }

    fn advance_eval_n(&mut self) {
        self.eval_n = (self.eval_n + 1) & 0x3F;
        if self.eval_n == 0 {
            self.eval_done = true;
        }
    }

    /// One dot of the sprite fetch interval (cycles 257-320). Slot i covers
    /// dots 257+8i to 264+8i: two garbage nametable fetches (the addresses
    /// the background pipeline would use), then the pattern low and high
    /// bytes, each read on the second dot of its pair with the next address
    /// driven at the end of that dot. Every slot fetches, even an empty one
    /// (tile $FF from the $FF-filled secondary OAM); the A12 rise for the
    /// first pattern fetch, at dot 260, is what clocks the MMC3 once per
    /// line when sprites use $1000.
    fn fetch_sprites_cycle(&mut self, mapper: &mut dyn Mapper) {
        let off = self.cycle - 257;
        let slot = (off / 8) as usize;
        match off % 8 {
            0 => {
                if slot == 0 {
                    // The units for the next line are loaded from here on;
                    // the current line's pixels are all out already.
                    self.sprite_count = (self.eval_sec_addr >> 2).min(8);
                    self.sprite_zero_in_units = self.sprite_zero_next;
                }
            }
            1 => {
                let addr = self.bg_fetch_addr(0);
                self.read_vram(addr, mapper);
                self.drive_addr_bus(self.bg_fetch_addr(1), mapper);
            }
            3 => {
                let addr = self.bg_fetch_addr(1);
                self.read_vram(addr, mapper);
                // Dot 260 + 8 * slot: the MMC3 clock when sprites use $1000.
                self.drive_addr_bus(self.sprite_pattern_addr(slot), mapper);
            }
            5 => {
                let addr = self.sprite_pattern_addr(slot);
                let low = self.read_vram(addr, mapper);
                self.sprite_patterns[slot].0 = low;
                self.drive_addr_bus(addr + 8, mapper);
            }
            7 => {
                let addr = self.sprite_pattern_addr(slot) + 8;
                let high = self.read_vram(addr, mapper);
                let attributes = self.secondary_oam[slot * 4 + 2];
                let (mut low, mut high) = (self.sprite_patterns[slot].0, high);
                if attributes & 0x40 != 0 {
                    low = reverse_byte(low);
                    high = reverse_byte(high);
                }
                self.sprite_patterns[slot] = (low, high);
                self.sprite_positions[slot] = self.secondary_oam[slot * 4 + 3];
                self.sprite_attributes[slot] = attributes;
                // Next slot's garbage nametable fetch, or at 320 the
                // background prefetch; either way A12 drops.
                self.drive_addr_bus(self.bg_fetch_addr(0), mapper);
            }
            _ => {}
        }
    }

    /// Pattern address of the row of secondary OAM sprite `slot` that the
    /// next line needs. Rows are masked to the sprite height, so an empty
    /// slot (Y = $FF, tile $FF, attributes $FF) yields a valid tile $FF
    /// address; the pre-render line uses the same path.
    fn sprite_pattern_addr(&self, slot: usize) -> u16 {
        let y = self.secondary_oam[slot * 4];
        let tile = self.secondary_oam[slot * 4 + 1];
        let attributes = self.secondary_oam[slot * 4 + 2];
        let height = self.sprite_height();
        // Evaluated on line N for line N+1; sprites appear at Y+1.
        let mut row = (self.scanline as u8).wrapping_sub(y) & (height - 1);
        if attributes & 0x80 != 0 {
            row = (height - 1) - row;
        }
        if height == 8 {
            let base = if self.ctrl.contains(PpuCtrl::SPRITE_PATTERN) {
                0x1000
            } else {
                0x0000
            };
            base | (tile as u16) << 4 | row as u16
        } else {
            let bank = (tile as u16 & 1) << 12;
            let tile = (tile & 0xFE) as u16 + if row >= 8 { 1 } else { 0 };
            bank | tile << 4 | (row & 7) as u16
        }
    }

    /// Output of the sprite units for pixel `x`: (palette index, behind
    /// background, is sprite 0). The lowest-numbered unit with an opaque
    /// pixel wins.
    fn get_sprite_pixel(&self, x: u16) -> (u8, bool, bool) {
        for i in 0..self.sprite_count.min(8) as usize {
            let sprite_x = self.sprite_positions[i] as u16;
            if x < sprite_x || x >= sprite_x + 8 {
                continue;
            }
            let bit = 7 - (x - sprite_x);
            let (low, high) = self.sprite_patterns[i];
            let pixel_value = ((high >> bit) & 1) << 1 | ((low >> bit) & 1);
            if pixel_value != 0 {
                let attributes = self.sprite_attributes[i];
                let palette = (attributes & 0x03) + 4; // Sprite palettes are 4-7
                let priority = (attributes & 0x20) != 0;
                let is_sprite_zero = i == 0 && self.sprite_zero_in_units;
                return ((palette << 2) | pixel_value, priority, is_sprite_zero);
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

/// The 64 NTSC system colours as RGB, indexed by a palette RAM entry.
pub const NES_PALETTE: [(u8, u8, u8); 64] = [
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
    fn pattern_reads_report_the_fetch_to_the_mapper_after_the_byte() {
        // MMC2 with distinct 4 KB banks for the $FD and $FE latch states:
        // a $2007 read of $0FE8 (tile $FE, high plane, row 0) returns the
        // byte from the pre-flip bank and flips latch 0 for the next read,
        // and a read of $0FD8 flips it back. This is the PPU-side proof
        // that `read_vram` calls `ppu_fetch` after `ppu_read`.
        let mut ppu = Ppu::new();
        let mut chr = Vec::new();
        for bank in 0u8..4 {
            chr.extend(vec![0x20 | bank; 0x1000]);
        }
        let mut mapper =
            crate::cartridge::mapper9::Mapper9::new(vec![0; 0x8000], chr, Mirroring::Vertical);
        mapper.cpu_write(0xB000, 1); // latch 0 = $FD -> bank 1
        mapper.cpu_write(0xC000, 2); // latch 0 = $FE -> bank 2 (power-on)

        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0000), 0x22);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0FD8), 0x22);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0000), 0x21);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0FE8), 0x21);
        assert_eq!(read_ppu_addr(&mut ppu, &mut mapper, 0x0000), 0x22);
        // The background fetch path goes through the same read.
        ppu.bg_next_tile_id = 0xFD;
        ppu.v = 0;
        ppu.fetch_pattern_high(&mut mapper); // $0FD8
        assert_eq!(mapper.ppu_peek(0x0000), 0x21);
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

    // -----------------------------------------------------------------
    // OAM ($2003/$2004) behaviour, issue 14
    // -----------------------------------------------------------------

    fn oam_ppu(scanline: u16, cycle: u16, mask: PpuMask) -> Ppu {
        let mut ppu = Ppu::new();
        ppu.scanline = scanline;
        ppu.cycle = cycle;
        ppu.mask = mask;
        ppu
    }

    fn oam_write(ppu: &mut Ppu, addr: u8, value: u8) {
        let mut mapper = Mapper3::new(vec![0; 0x8000], tagged_chr(1), Mirroring::Vertical);
        ppu.write_register(0x2003, addr, &mut mapper);
        ppu.write_register(0x2004, value, &mut mapper);
    }

    fn oam_read(ppu: &mut Ppu, addr: u8) -> u8 {
        let mut mapper = Mapper3::new(vec![0; 0x8000], tagged_chr(1), Mirroring::Vertical);
        ppu.write_register(0x2003, addr, &mut mapper);
        ppu.read_register(0x2004, &mut mapper)
    }

    #[test]
    fn oam_attribute_bytes_drop_bits_2_to_4() {
        let mut ppu = oam_ppu(241, 10, PpuMask::empty());
        for addr in 0u8..8 {
            oam_write(&mut ppu, addr, 0xFF);
        }
        for addr in 0u8..8 {
            let expected = if addr & 3 == 2 { 0xE3 } else { 0xFF };
            assert_eq!(oam_read(&mut ppu, addr), expected, "OAM[{addr}]");
        }
    }

    #[test]
    fn oam_write_in_vblank_stores_and_increments_by_one() {
        let mut ppu = oam_ppu(241, 10, PpuMask::SHOW_BG | PpuMask::SHOW_SPRITES);
        oam_write(&mut ppu, 0x13, 0x42);
        assert_eq!(ppu.oam_addr, 0x14);
        assert_eq!(oam_read(&mut ppu, 0x13), 0x42);
        assert_eq!(ppu.oam_addr, 0x13, "reads do not move OAMADDR");
    }

    #[test]
    fn oam_write_with_rendering_disabled_is_normal_on_visible_line() {
        let mut ppu = oam_ppu(100, 30, PpuMask::empty());
        oam_write(&mut ppu, 0x20, 0x99);
        assert_eq!(ppu.oam_addr, 0x21);
        assert_eq!(oam_read(&mut ppu, 0x20), 0x99);
    }

    #[test]
    fn oam_read_returns_ff_during_secondary_oam_clear() {
        let mut ppu = oam_ppu(241, 10, PpuMask::empty());
        oam_write(&mut ppu, 0x05, 0x37);
        ppu.mask = PpuMask::SHOW_SPRITES;

        for (scanline, cycle) in [(0, 1), (100, 32), (239, 64), (261, 10)] {
            ppu.scanline = scanline;
            ppu.cycle = cycle;
            assert_eq!(
                oam_read(&mut ppu, 0x05),
                0xFF,
                "line {scanline} cycle {cycle}"
            );
        }

        ppu.scanline = 241;
        ppu.cycle = 32;
        assert_eq!(oam_read(&mut ppu, 0x05), 0x37, "vblank");
        ppu.scanline = 100;
        ppu.cycle = 32;
        ppu.mask = PpuMask::empty();
        assert_eq!(oam_read(&mut ppu, 0x05), 0x37, "rendering off");
    }

    #[test]
    fn oam_write_during_rendering_is_ignored_with_glitchy_increment() {
        let mut ppu = oam_ppu(241, 10, PpuMask::empty());
        oam_write(&mut ppu, 0x07, 0x11);
        ppu.mask = PpuMask::SHOW_BG;
        ppu.scanline = 50;
        ppu.cycle = 200;

        oam_write(&mut ppu, 0x07, 0xEE);
        assert_eq!(ppu.oam_addr, 0x0B, "bits 2-7 advance by one sprite");
        ppu.oam_addr = 0xFE;
        oam_write(&mut ppu, 0xFE, 0xEE);
        assert_eq!(ppu.oam_addr, 0x02, "wraps without touching bits 0-1");

        ppu.mask = PpuMask::empty();
        assert_eq!(oam_read(&mut ppu, 0x07), 0x11, "data was not stored");
    }

    // -----------------------------------------------------------------
    // I/O bus (open bus) latch
    // -----------------------------------------------------------------

    fn nrom() -> Box<dyn Mapper> {
        Cartridge::build_mapper(0, vec![0; 0x8000], vec![], Mirroring::Vertical)
    }

    #[test]
    fn write_loads_latch_and_write_only_reads_return_it() {
        let mut ppu = Ppu::new();
        let mut m = nrom();
        for (reg, value) in [
            (0x2000u16, 0x55u8),
            (0x2001, 0xAA),
            (0x2002, 0x12),
            (0x2005, 0x34),
        ] {
            ppu.write_register(reg, value, m.as_mut());
            for r in [0x2000u16, 0x2001, 0x2003, 0x2005, 0x2006] {
                assert_eq!(ppu.read_register(r, m.as_mut()), value, "reg {r:04X}");
            }
        }
    }

    #[test]
    fn status_drives_top_three_bits_only() {
        let mut ppu = Ppu::new();
        let mut m = nrom();
        ppu.write_register(0x2003, 0x1F, m.as_mut());
        ppu.status = PpuStatus::VBLANK_STARTED | PpuStatus::SPRITE_ZERO_HIT;
        assert_eq!(ppu.read_register(0x2002, m.as_mut()), 0xC0 | 0x1F);
        // Flags loaded into the latch, low bits untouched.
        assert_eq!(ppu.read_register(0x2000, m.as_mut()), 0xC0 | 0x1F);

        // Low bits were not refreshed by the $2002 read: they decay from
        // the write, while the flag bits decay from the later read.
        ppu.frame += IO_BUS_DECAY_FRAMES;
        ppu.status = PpuStatus::empty();
        assert_eq!(ppu.read_register(0x2002, m.as_mut()), 0x1F);
        ppu.frame += 1;
        assert_eq!(ppu.read_register(0x2000, m.as_mut()), 0x00);
    }

    #[test]
    fn oam_read_refreshes_all_bits() {
        let mut ppu = Ppu::new();
        let mut m = nrom();
        ppu.oam_data[0] = 0xC3;
        ppu.write_register(0x2003, 0x00, m.as_mut());
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 0xC3);
        assert_eq!(ppu.read_register(0x2005, m.as_mut()), 0xC3);
    }

    #[test]
    fn vram_read_sets_latch_and_palette_read_keeps_top_bits() {
        let mut ppu = Ppu::new();
        let mut m = nrom();
        write_ppu_addr(&mut ppu, m.as_mut(), 0x2100, 0x96);
        assert_eq!(read_ppu_addr(&mut ppu, m.as_mut(), 0x2100), 0x96);
        assert_eq!(ppu.read_register(0x2000, m.as_mut()), 0x96);

        write_ppu_addr(&mut ppu, m.as_mut(), 0x3F05, 0xFF);
        ppu.write_register(0x2006, 0x3F, m.as_mut());
        ppu.write_register(0x2006, 0x05, m.as_mut());
        // Latch is $05 from the last write: bits 7-6 clear.
        assert_eq!(ppu.read_register(0x2007, m.as_mut()), 0x3F);
        ppu.write_register(0x2006, 0x3F, m.as_mut());
        ppu.write_register(0x2006, 0x85, m.as_mut());
        // Latch is $85: bit 7 shows over the 6-bit palette entry.
        assert_eq!(ppu.read_register(0x2007, m.as_mut()), 0xBF);
        // Bits 7-6 were not refreshed by the palette read.
        ppu.frame += IO_BUS_DECAY_FRAMES + 1;
        assert_eq!(ppu.read_register(0x2000, m.as_mut()), 0x00);
    }

    #[test]
    fn latch_decays_per_bit() {
        let mut ppu = Ppu::new();
        let mut m = nrom();
        ppu.write_register(0x2000, 0xFF, m.as_mut());
        ppu.frame += IO_BUS_DECAY_FRAMES;
        assert_eq!(ppu.read_register(0x2001, m.as_mut()), 0xFF);
        // Refresh bits 7-5 only (VBL clear, so they refresh to 0).
        ppu.status = PpuStatus::SPRITE_OVERFLOW;
        assert_eq!(ppu.read_register(0x2002, m.as_mut()), 0x3F);
        ppu.frame += 1;
        assert_eq!(ppu.read_register(0x2001, m.as_mut()), 0x20);
        ppu.frame += IO_BUS_DECAY_FRAMES + 1;
        assert_eq!(ppu.read_register(0x2001, m.as_mut()), 0x00);
    }

    // -----------------------------------------------------------------
    // Sprite pipeline (issue 11): evaluation cadence, overflow bug,
    // sprite 0 hit clipping
    // -----------------------------------------------------------------

    /// A PPU parked at the end of the secondary OAM clear on `scanline`,
    /// with background rendering on so the pipeline runs.
    fn eval_ppu(scanline: u16) -> Ppu {
        let mut ppu = Ppu::new();
        ppu.mask = PpuMask::SHOW_BG;
        ppu.scanline = scanline;
        ppu.cycle = 64;
        ppu.secondary_oam = [0xFF; 32];
        ppu
    }

    fn set_sprite(ppu: &mut Ppu, index: usize, y: u8, tile: u8, attr: u8, x: u8) {
        ppu.oam_data[index * 4..index * 4 + 4].copy_from_slice(&[y, tile, attr, x]);
    }

    /// Step until `cycle` has just been processed.
    fn step_to(ppu: &mut Ppu, m: &mut dyn Mapper, cycle: u16) {
        while ppu.cycle < cycle {
            ppu.step(m);
        }
    }

    #[test]
    fn evaluation_reads_oam_on_odd_dots_and_copies_in_range_sprites() {
        let mut ppu = eval_ppu(10);
        let mut m = nrom();
        set_sprite(&mut ppu, 0, 10, 0x11, 0x01, 0x40); // rows 11-18: in range
        set_sprite(&mut ppu, 1, 100, 0x22, 0x02, 0x50); // out of range
        set_sprite(&mut ppu, 2, 5, 0x33, 0x03, 0x60); // rows 6-13: in range
        for i in 3..64 {
            set_sprite(&mut ppu, i, 0xF0, 0, 0, 0);
        }

        // 65: read sprite 0 Y; visible through $2004.
        step_to(&mut ppu, m.as_mut(), 65);
        assert_eq!(ppu.oam_copy_buffer, 10);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 10);
        assert_eq!(ppu.oam_addr, 0, "OAMADDR moves on the write dot");
        // 66: write it to secondary OAM, m advances.
        step_to(&mut ppu, m.as_mut(), 66);
        assert_eq!(ppu.secondary_oam[0], 10);
        assert_eq!(ppu.oam_addr, 1);
        // 72: four bytes copied, on to sprite 1.
        step_to(&mut ppu, m.as_mut(), 72);
        assert_eq!(&ppu.secondary_oam[0..4], &[10, 0x11, 0x01, 0x40]);
        assert_eq!(ppu.oam_addr, 4);
        assert!(ppu.sprite_zero_next);
        // 73/74: sprite 1 is out of range, two dots and n advances.
        step_to(&mut ppu, m.as_mut(), 73);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 100);
        step_to(&mut ppu, m.as_mut(), 74);
        assert_eq!(ppu.oam_addr, 8);
        assert_eq!(ppu.secondary_oam[4], 100, "Y is written even on a miss");
        // 82: sprite 2 copied into slot 1 (overwriting the miss).
        step_to(&mut ppu, m.as_mut(), 82);
        assert_eq!(&ppu.secondary_oam[4..8], &[5, 0x33, 0x03, 0x60]);
        assert_eq!(ppu.oam_addr, 12);

        step_to(&mut ppu, m.as_mut(), 256);
        assert_eq!(ppu.eval_sec_addr, 8);
        assert!(!ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));

        // 257: units latch and OAMADDR is held at 0 through the fetches;
        // $2004 shows the secondary OAM byte the fetch is using.
        ppu.oam_addr = 0x55;
        step_to(&mut ppu, m.as_mut(), 257);
        assert_eq!(ppu.oam_addr, 0);
        assert_eq!(ppu.sprite_count, 2);
        assert!(ppu.sprite_zero_in_units);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 10);
        step_to(&mut ppu, m.as_mut(), 266);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 0x33);
        step_to(&mut ppu, m.as_mut(), 268);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 0x60);
        step_to(&mut ppu, m.as_mut(), 320);
        assert_eq!(ppu.sprite_positions[1], 0x60);
        assert_eq!(ppu.sprite_attributes[1], 0x03);
    }

    #[test]
    fn evaluation_runs_with_background_only_and_not_on_pre_render_line() {
        let mut ppu = eval_ppu(20);
        let mut m = nrom();
        for i in 0..9 {
            set_sprite(&mut ppu, i, 20, 0, 0, 0);
        }
        step_to(&mut ppu, m.as_mut(), 256);
        assert!(ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));

        let mut ppu = eval_ppu(261);
        for i in 0..9 {
            set_sprite(&mut ppu, i, 5, 0, 0, 0);
        }
        step_to(&mut ppu, m.as_mut(), 320);
        assert!(!ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));
        assert_eq!(ppu.sprite_count, 0);
        assert_eq!(ppu.oam_addr, 0);
    }

    #[test]
    fn ninth_sprite_sets_overflow_on_its_write_dot() {
        let mut ppu = eval_ppu(30);
        let mut m = nrom();
        for i in 0..9 {
            set_sprite(&mut ppu, i, 30, 0, 0, 0);
        }
        // Sprites 0-7 take dots 65-128; sprite 8's Y is read at 129.
        step_to(&mut ppu, m.as_mut(), 129);
        assert!(!ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));
        step_to(&mut ppu, m.as_mut(), 130);
        assert!(ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));
        // Its remaining three bytes are read with m carrying into n, then
        // evaluation only walks n.
        step_to(&mut ppu, m.as_mut(), 136);
        assert!(ppu.eval_done);
        assert_eq!(ppu.oam_addr, 9 << 2);
    }

    #[test]
    fn overflow_scan_bug_reads_the_wrong_byte_as_y() {
        // Eight sprites in range, the ninth not: the scan then compares
        // byte 1 of sprite 9, byte 2 of sprite 10, ... as Y coordinates.
        let mut m = nrom();
        let mut ppu = eval_ppu(30);
        for i in 0..8 {
            set_sprite(&mut ppu, i, 30, 0, 0, 0);
        }
        for i in 8..64 {
            set_sprite(&mut ppu, i, 200, 200, 0xE0, 200);
        }
        set_sprite(&mut ppu, 9, 200, 30, 0xE0, 200); // tile byte in range
        step_to(&mut ppu, m.as_mut(), 131);
        assert_eq!(ppu.read_register(0x2004, m.as_mut()), 30, "reads OAM[9][1]");
        step_to(&mut ppu, m.as_mut(), 132);
        assert!(ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));

        // Same layout but byte 1 of sprite 9 out of range and byte 3 of
        // sprite 11 in range (X = 30): still flagged.
        let mut ppu = eval_ppu(30);
        for i in 0..8 {
            set_sprite(&mut ppu, i, 30, 0, 0, 0);
        }
        for i in 8..64 {
            set_sprite(&mut ppu, i, 200, 200, 0xE0, 200);
        }
        set_sprite(&mut ppu, 11, 200, 200, 0xE0, 30);
        step_to(&mut ppu, m.as_mut(), 256);
        assert!(ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));

        // And with every wrongly-read byte out of range: no flag, even
        // though sprite 13's real Y is in range (the scan reads its tile
        // byte; sprite 12 is the one whose real Y gets looked at).
        let mut ppu = eval_ppu(30);
        for i in 0..8 {
            set_sprite(&mut ppu, i, 30, 0, 0, 0);
        }
        for i in 8..64 {
            set_sprite(&mut ppu, i, 200, 200, 0xE0, 200);
        }
        set_sprite(&mut ppu, 13, 30, 200, 0xE0, 200);
        step_to(&mut ppu, m.as_mut(), 256);
        assert!(!ppu.status.contains(PpuStatus::SPRITE_OVERFLOW));
    }

    /// Sprite 0 with a solid pattern at `x`, over a solid background.
    fn hit_ppu(x: u8, mask: PpuMask) -> Ppu {
        let mut ppu = Ppu::new();
        ppu.mask = mask;
        ppu.scanline = 50;
        ppu.sprite_count = 1;
        ppu.sprite_zero_in_units = true;
        ppu.sprite_patterns[0] = (0xFF, 0x00);
        ppu.sprite_positions[0] = x;
        ppu.sprite_attributes[0] = 0;
        ppu.bg_shift_pattern_lo = 0xFFFF;
        ppu.bg_shift_attrib_lo = 0xFFFF;
        ppu
    }

    fn hit_at(ppu: &mut Ppu, x: u16) -> bool {
        ppu.status.remove(PpuStatus::SPRITE_ZERO_HIT);
        ppu.cycle = x + 1;
        ppu.render_pixel();
        ppu.status.contains(PpuStatus::SPRITE_ZERO_HIT)
    }

    #[test]
    fn sprite_zero_hit_needs_both_layers_opaque_and_enabled() {
        let both = PpuMask::SHOW_BG | PpuMask::SHOW_SPRITES;
        let mut ppu = hit_ppu(100, both);
        assert!(hit_at(&mut ppu, 100));
        assert!(!hit_at(&mut ppu, 99), "left of the sprite");
        assert!(hit_at(&mut ppu, 107));
        assert!(!hit_at(&mut ppu, 108), "right of the sprite");

        ppu.sprite_attributes[0] = 0x20;
        assert!(hit_at(&mut ppu, 100), "priority does not matter");
        ppu.sprite_zero_in_units = false;
        assert!(!hit_at(&mut ppu, 100), "unit 0 is not sprite 0");
        ppu.sprite_zero_in_units = true;
        ppu.bg_shift_pattern_lo = 0;
        assert!(!hit_at(&mut ppu, 100), "transparent background");

        let mut ppu = hit_ppu(100, PpuMask::SHOW_SPRITES);
        assert!(!hit_at(&mut ppu, 100), "background disabled");
        let mut ppu = hit_ppu(100, PpuMask::SHOW_BG);
        assert!(!hit_at(&mut ppu, 100), "sprites disabled");
    }

    #[test]
    fn sprite_zero_hit_respects_left_clip_and_column_255() {
        let both = PpuMask::SHOW_BG | PpuMask::SHOW_SPRITES;
        let mut ppu = hit_ppu(3, both);
        for x in 3..8 {
            assert!(!hit_at(&mut ppu, x), "x={x} under the clip");
        }
        assert!(hit_at(&mut ppu, 8), "first unclipped column");

        let mut ppu = hit_ppu(3, both | PpuMask::SHOW_BG_LEFT);
        assert!(!hit_at(&mut ppu, 5), "sprites still clipped");
        let mut ppu = hit_ppu(3, both | PpuMask::SHOW_SPRITES_LEFT);
        assert!(!hit_at(&mut ppu, 5), "background still clipped");
        let mut ppu = hit_ppu(3, both | PpuMask::SHOW_BG_LEFT | PpuMask::SHOW_SPRITES_LEFT);
        assert!(hit_at(&mut ppu, 3), "no clipping");

        let mut ppu = hit_ppu(250, both);
        assert!(hit_at(&mut ppu, 254));
        assert!(!hit_at(&mut ppu, 255), "column 255 never hits");
    }

    #[test]
    fn sprite_fetch_a12_rises_at_dot_260_and_bg_prefetch_at_324() {
        // Mapper 4 counts filtered A12 rises; with count 0 and IRQ enabled
        // every clock sets the flag, so it marks the dot of each rise.
        let mut m = Mapper4::new(vec![0; 0x8000], tagged_chr(1), Mirroring::Vertical);
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);

        let mut ppu = Ppu::new();
        ppu.mask = PpuMask::SHOW_BG;
        ppu.scanline = 100;
        ppu.cycle = 0;
        // Sprites at $1000, background at $0000.
        ppu.ctrl = PpuCtrl::SPRITE_PATTERN;
        step_to(&mut ppu, &mut m, 259);
        assert!(!m.irq_pending());
        ppu.step(&mut m);
        assert_eq!(ppu.cycle, 260);
        assert!(m.irq_pending(), "clocked by the first sprite pattern fetch");
        m.clear_irq();
        step_to(&mut ppu, &mut m, 340);
        assert!(!m.irq_pending(), "later slots are inside the filter window");

        // Background at $1000, sprites at $0000: the rise is the prefetch
        // of the next line's first tile, and the tile fetches at dot 4 of
        // the following line are still inside the filter window.
        let mut ppu = Ppu::new();
        ppu.mask = PpuMask::SHOW_BG;
        ppu.scanline = 100;
        ppu.cycle = 0;
        ppu.ctrl = PpuCtrl::BG_PATTERN;
        step_to(&mut ppu, &mut m, 256);
        m.clear_irq();
        step_to(&mut ppu, &mut m, 323);
        assert!(!m.irq_pending());
        ppu.step(&mut m);
        assert_eq!(ppu.cycle, 324);
        assert!(m.irq_pending());
        m.clear_irq();
        step_to(&mut ppu, &mut m, 340);
        ppu.step(&mut m);
        assert_eq!((ppu.scanline, ppu.cycle), (101, 0));
        step_to(&mut ppu, &mut m, 256);
        assert!(!m.irq_pending(), "no clock on the next line before 324");
    }
}

// ----------------------------------------------------------------------
// Save states (docs/debugging/SAVE_STATES.md). Section "PPU ". The frame
// buffer is not part of the state: a state is taken between frames and
// the next frame redraws it (while rendering is enabled).
// ----------------------------------------------------------------------

impl crate::state::Snapshot for Ppu {
    fn save(&self, w: &mut crate::state::Writer) {
        // Registers.
        w.u8(self.ctrl.bits());
        w.u8(self.mask.bits());
        w.u8(self.status.bits());
        w.u8(self.oam_addr);
        w.bytes(&self.oam_data);
        w.u8(self.ppu_data_buffer);
        w.bytes(&self.nametable_ram);
        w.bytes(&self.palette);
        // Position.
        w.u16(self.scanline);
        w.u16(self.cycle);
        w.u64(self.frame);
        // A12 filter.
        w.bool(self.a12_last);
        w.u16(self.a12_low_cycles);
        // I/O bus latch.
        w.u8(self.io_bus);
        for stamp in &self.io_bus_stamp {
            w.u64(*stamp);
        }
        // NMI.
        w.bool(self.nmi_interrupt);
        w.bool(self.nmi_line);
        w.bool(self.suppress_vbl);
        // Scroll registers.
        w.u16(self.v);
        w.u16(self.t);
        w.u8(self.x);
        w.bool(self.w);
        // Background pipeline.
        w.u16(self.bg_shift_pattern_lo);
        w.u16(self.bg_shift_pattern_hi);
        w.u16(self.bg_shift_attrib_lo);
        w.u16(self.bg_shift_attrib_hi);
        w.u8(self.bg_next_tile_id);
        w.u8(self.bg_next_tile_attrib);
        w.u8(self.bg_next_tile_lsb);
        w.u8(self.bg_next_tile_msb);
        // Sprite evaluation.
        w.bytes(&self.secondary_oam);
        w.u8(self.oam_copy_buffer);
        w.u8(self.eval_n);
        w.u8(self.eval_m);
        w.u8(self.eval_sec_addr);
        w.bool(self.eval_in_range);
        w.bool(self.eval_done);
        w.u8(self.eval_overflow_reads);
        w.bool(self.eval_first);
        w.bool(self.sprite_zero_next);
        // Sprite render units.
        w.u8(self.sprite_count);
        w.bool(self.sprite_zero_in_units);
        for (lo, hi) in &self.sprite_patterns {
            w.u8(*lo);
            w.u8(*hi);
        }
        w.bytes(&self.sprite_positions);
        w.bytes(&self.sprite_attributes);
    }

    fn load(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.ctrl = PpuCtrl::from_bits_retain(r.u8()?);
        self.mask = PpuMask::from_bits_retain(r.u8()?);
        self.status = PpuStatus::from_bits_retain(r.u8()?);
        self.oam_addr = r.u8()?;
        r.bytes(&mut self.oam_data)?;
        self.ppu_data_buffer = r.u8()?;
        r.bytes(&mut self.nametable_ram)?;
        r.bytes(&mut self.palette)?;
        self.scanline = r.u16()?;
        self.cycle = r.u16()?;
        self.frame = r.u64()?;
        self.a12_last = r.bool()?;
        self.a12_low_cycles = r.u16()?;
        self.io_bus = r.u8()?;
        for stamp in self.io_bus_stamp.iter_mut() {
            *stamp = r.u64()?;
        }
        self.nmi_interrupt = r.bool()?;
        self.nmi_line = r.bool()?;
        self.suppress_vbl = r.bool()?;
        self.v = r.u16()?;
        self.t = r.u16()?;
        self.x = r.u8()?;
        self.w = r.bool()?;
        self.bg_shift_pattern_lo = r.u16()?;
        self.bg_shift_pattern_hi = r.u16()?;
        self.bg_shift_attrib_lo = r.u16()?;
        self.bg_shift_attrib_hi = r.u16()?;
        self.bg_next_tile_id = r.u8()?;
        self.bg_next_tile_attrib = r.u8()?;
        self.bg_next_tile_lsb = r.u8()?;
        self.bg_next_tile_msb = r.u8()?;
        r.bytes(&mut self.secondary_oam)?;
        self.oam_copy_buffer = r.u8()?;
        self.eval_n = r.u8()?;
        self.eval_m = r.u8()?;
        self.eval_sec_addr = r.u8()?;
        self.eval_in_range = r.bool()?;
        self.eval_done = r.bool()?;
        self.eval_overflow_reads = r.u8()?;
        self.eval_first = r.bool()?;
        self.sprite_zero_next = r.bool()?;
        self.sprite_count = r.u8()?;
        self.sprite_zero_in_units = r.bool()?;
        for pattern in self.sprite_patterns.iter_mut() {
            *pattern = (r.u8()?, r.u8()?);
        }
        r.bytes(&mut self.sprite_positions)?;
        r.bytes(&mut self.sprite_attributes)?;
        Ok(())
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::state::{Reader, Snapshot, Writer};

    /// Fill every field with a distinct value so a swapped or missed field
    /// shows up in the comparison.
    fn busy_ppu() -> Ppu {
        let mut p = Ppu::new();
        p.ctrl = PpuCtrl::from_bits_retain(0x91);
        p.mask = PpuMask::from_bits_retain(0x1E);
        p.status = PpuStatus::from_bits_retain(0xC0);
        p.oam_addr = 0x24;
        for (i, b) in p.oam_data.iter_mut().enumerate() {
            *b = i as u8 ^ 0x5A;
        }
        p.ppu_data_buffer = 0x77;
        for (i, b) in p.nametable_ram.iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        for (i, b) in p.palette.iter_mut().enumerate() {
            *b = i as u8 + 0x10;
        }
        p.scanline = 123;
        p.cycle = 321;
        p.frame = 0x1122_3344_5566;
        p.a12_last = true;
        p.a12_low_cycles = 9;
        p.io_bus = 0xA5;
        p.io_bus_stamp = [1, 2, 3, 4, 5, 6, 7, 8];
        p.nmi_interrupt = true;
        p.nmi_line = true;
        p.suppress_vbl = true;
        p.v = 0x2C1F;
        p.t = 0x0C00;
        p.x = 5;
        p.w = true;
        p.bg_shift_pattern_lo = 0x1234;
        p.bg_shift_pattern_hi = 0x5678;
        p.bg_shift_attrib_lo = 0x9ABC;
        p.bg_shift_attrib_hi = 0xDEF0;
        p.bg_next_tile_id = 0x11;
        p.bg_next_tile_attrib = 0x22;
        p.bg_next_tile_lsb = 0x33;
        p.bg_next_tile_msb = 0x44;
        for (i, b) in p.secondary_oam.iter_mut().enumerate() {
            *b = 0x80 | i as u8;
        }
        p.oam_copy_buffer = 0x66;
        p.eval_n = 12;
        p.eval_m = 2;
        p.eval_sec_addr = 20;
        p.eval_in_range = true;
        p.eval_done = true;
        p.eval_overflow_reads = 3;
        p.eval_first = true;
        p.sprite_zero_next = true;
        p.sprite_count = 6;
        p.sprite_zero_in_units = true;
        for (i, s) in p.sprite_patterns.iter_mut().enumerate() {
            *s = (i as u8, 0xF0 | i as u8);
        }
        p.sprite_positions = [10, 20, 30, 40, 50, 60, 70, 80];
        p.sprite_attributes = [1, 2, 3, 4, 5, 6, 7, 8];
        p
    }

    fn image(p: &Ppu) -> Vec<u8> {
        let mut w = Writer::new();
        p.save(&mut w);
        w.into_bytes()
    }

    #[test]
    fn ppu_round_trips_every_field() {
        let original = busy_ppu();
        let bytes = image(&original);
        let mut restored = Ppu::new();
        let mut r = Reader::new(&bytes);
        restored.load(&mut r).unwrap();
        assert_eq!(r.remaining(), 0, "load consumed the whole image");
        assert_eq!(image(&restored), bytes);
        assert_eq!(restored.v, 0x2C1F);
        assert_eq!(restored.scanline, 123);
        assert_eq!(restored.sprite_patterns[7], (7, 0xF7));
        assert_eq!(restored.io_bus_stamp[7], 8);
        assert!(restored.nmi_interrupt && restored.suppress_vbl);
    }

    #[test]
    fn ppu_truncated_image_is_refused() {
        let bytes = image(&busy_ppu());
        let mut restored = Ppu::new();
        let cut = &bytes[..bytes.len() - 1];
        assert_eq!(
            restored.load(&mut Reader::new(cut)),
            Err(crate::state::StateError::Truncated)
        );
    }
}
