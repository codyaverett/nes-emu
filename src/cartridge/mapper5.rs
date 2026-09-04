//! Mapper 5 (MMC5 / ExROM).
//!
//! PRG and CHR banking, the multiplier, ExRAM and the register file are
//! implemented. ExRAM-as-nametable, fill mode, the vertical split and the
//! scanline IRQ are only partially modelled: the PPU has no hook for
//! per-table nametable sources yet, so `mirroring()` approximates $5105 with
//! the closest standard mirroring mode.

use super::mapper::{Chr, Mapper};
use super::Mirroring;

pub struct Mapper5 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],

    // ExRAM - 1KB internal RAM for extended attributes and nametables
    exram: [u8; 0x400],
    exram_mode: u8,

    // PRG banking
    prg_mode: u8,
    prg_banks: [u8; 5],
    #[allow(dead_code)] // write protection is recorded but not enforced yet
    prg_ram_protect: [u8; 2],

    // CHR banking
    chr_mode: u8,
    chr_banks: [u16; 12],
    upper_chr_bank_bits: u8,

    // Nametable control
    nametable_mapping: [u8; 4],
    #[allow(dead_code)] // fill mode needs a PPU nametable hook
    fill_mode_tile: u8,
    #[allow(dead_code)]
    fill_mode_attr: u8,

    // Split screen control (recorded, not rendered)
    #[allow(dead_code)]
    vsplit_enabled: bool,
    #[allow(dead_code)]
    vsplit_side: bool,
    #[allow(dead_code)]
    vsplit_tile: u8,
    #[allow(dead_code)]
    vsplit_scroll: u8,
    #[allow(dead_code)]
    vsplit_bank: u8,

    // IRQ control
    irq_scanline: u8,
    irq_enabled: bool,
    irq_pending: bool,
    irq_in_frame: bool,
    scanline_counter: u8,

    // PPU monitoring
    ppu_is_rendering: bool,

    // Multiplication unit
    multiplicand_a: u8,
    multiplicand_b: u8,
}

impl Mapper5 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, header_mirroring: Mirroring) -> Self {
        let nametable_mapping = match header_mirroring {
            Mirroring::Vertical => [0, 1, 0, 1],
            Mirroring::Horizontal => [0, 0, 1, 1],
            Mirroring::SingleScreenLower => [0, 0, 0, 0],
            Mirroring::SingleScreenUpper => [1, 1, 1, 1],
            Mirroring::FourScreen => [0, 1, 2, 3],
        };

        let mut mapper = Mapper5 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            exram: [0; 0x400],
            exram_mode: 0,

            prg_mode: 3,
            prg_banks: [0xFF; 5],
            prg_ram_protect: [0; 2],

            chr_mode: 0,
            chr_banks: [0; 12],
            upper_chr_bank_bits: 0,

            nametable_mapping,
            fill_mode_tile: 0,
            fill_mode_attr: 0,

            vsplit_enabled: false,
            vsplit_side: false,
            vsplit_tile: 0,
            vsplit_scroll: 0,
            vsplit_bank: 0,

            irq_scanline: 0,
            irq_enabled: false,
            irq_pending: false,
            irq_in_frame: false,
            scanline_counter: 0,

            ppu_is_rendering: false,

            multiplicand_a: 0,
            multiplicand_b: 0,
        };

        // Initialize the fixed slot to point at the last bank
        let last_bank = (mapper.prg_rom.len() / 0x2000).saturating_sub(1) as u8;
        mapper.prg_banks[4] = last_bank;

        mapper
    }

    fn prg_read_at(&self, offset: usize) -> u8 {
        if offset < self.prg_rom.len() {
            self.prg_rom[offset]
        } else {
            0
        }
    }

    /// `rel` is the offset from $8000.
    fn read_prg(&self, rel: u16) -> u8 {
        match self.prg_mode {
            0 => {
                // Mode 0: 32KB switchable
                let bank = (self.prg_banks[4] >> 2) as usize;
                self.prg_read_at(bank * 0x8000 + rel as usize)
            }
            1 => {
                // Mode 1: 16KB + 16KB
                if rel < 0x4000 {
                    let bank = (self.prg_banks[2] >> 1) as usize;
                    self.prg_read_at(bank * 0x4000 + rel as usize)
                } else {
                    let bank = (self.prg_banks[4] >> 1) as usize;
                    self.prg_read_at(bank * 0x4000 + (rel - 0x4000) as usize)
                }
            }
            2 => {
                // Mode 2: 16KB + 8KB + 8KB
                match rel {
                    0x0000..=0x3FFF => {
                        let bank = (self.prg_banks[2] >> 1) as usize;
                        self.prg_read_at(bank * 0x4000 + rel as usize)
                    }
                    0x4000..=0x5FFF => {
                        let bank = self.prg_banks[3] as usize;
                        self.prg_read_at(bank * 0x2000 + (rel - 0x4000) as usize)
                    }
                    _ => {
                        let bank = self.prg_banks[4] as usize;
                        self.prg_read_at(bank * 0x2000 + (rel - 0x6000) as usize)
                    }
                }
            }
            _ => {
                // Mode 3: 8KB + 8KB + 8KB + 8KB
                let bank_index = (rel / 0x2000) as usize;
                let bank = self.prg_banks[bank_index + 1] as usize;
                self.prg_read_at(bank * 0x2000 + (rel & 0x1FFF) as usize)
            }
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let addr = addr & 0x1FFF;
        let (bank_index, bank_size): (usize, usize) = match self.chr_mode {
            0 => ((addr / 0x2000) as usize * 8, 0x2000),
            1 => ((addr / 0x1000) as usize * 4, 0x1000),
            2 => ((addr / 0x800) as usize * 2, 0x800),
            _ => ((addr / 0x400) as usize, 0x400),
        };
        // In the wider modes the register that selects a bank is the last
        // register of the group (e.g. $5127 for 8 KB mode).
        let reg = (bank_index + bank_size / 0x400 - 1).min(11);
        let bank = self.chr_banks[reg] as usize;
        bank * bank_size + (addr as usize & (bank_size - 1))
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5015 => {
                // Audio registers (not implemented)
            }
            0x5100 => self.prg_mode = value & 0x03,
            0x5101 => self.chr_mode = value & 0x03,
            0x5102 => self.prg_ram_protect[0] = value & 0x03,
            0x5103 => self.prg_ram_protect[1] = value & 0x03,
            0x5104 => self.exram_mode = value & 0x03,
            0x5105 => {
                self.nametable_mapping[0] = value & 0x03;
                self.nametable_mapping[1] = (value >> 2) & 0x03;
                self.nametable_mapping[2] = (value >> 4) & 0x03;
                self.nametable_mapping[3] = (value >> 6) & 0x03;
            }
            0x5106 => self.fill_mode_tile = value,
            0x5107 => self.fill_mode_attr = value & 0x03,
            0x5113..=0x5117 => {
                let bank_index = (addr - 0x5113) as usize;
                self.prg_banks[bank_index] = value & 0x7F;
            }
            0x5120..=0x512B => {
                let bank_index = (addr - 0x5120) as usize;
                self.chr_banks[bank_index] =
                    value as u16 | ((self.upper_chr_bank_bits as u16) << 8);
            }
            0x5130 => self.upper_chr_bank_bits = value & 0x03,
            0x5200 => {
                self.vsplit_enabled = (value & 0x80) != 0;
                self.vsplit_side = (value & 0x40) != 0;
                self.vsplit_tile = value & 0x1F;
            }
            0x5201 => self.vsplit_scroll = value,
            0x5202 => self.vsplit_bank = value,
            0x5203 => self.irq_scanline = value,
            0x5204 => self.irq_enabled = (value & 0x80) != 0,
            0x5205 => self.multiplicand_a = value,
            0x5206 => self.multiplicand_b = value,
            // ExRAM mode 3 is read-only
            0x5C00..=0x5FFF if self.exram_mode < 3 => {
                self.exram[(addr - 0x5C00) as usize] = value;
            }
            _ => {}
        }
    }

    fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x5204 => {
                let status = ((self.irq_pending as u8) << 7) | ((self.irq_in_frame as u8) << 6);
                self.irq_pending = false;
                status
            }
            0x5205 => (self.get_multiplication_result() & 0xFF) as u8,
            0x5206 => (self.get_multiplication_result() >> 8) as u8,
            0x5C00..=0x5FFF => self.exram[(addr - 0x5C00) as usize],
            _ => 0,
        }
    }

    pub fn get_multiplication_result(&self) -> u16 {
        (self.multiplicand_a as u16) * (self.multiplicand_b as u16)
    }

    pub fn notify_ppu_state(&mut self, rendering: bool) {
        self.ppu_is_rendering = rendering;
        if !rendering {
            self.scanline_counter = 0;
            self.irq_in_frame = false;
        }
    }
}

impl Mapper for Mapper5 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x5000..=0x5FFF => self.read_register(addr),
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => self.read_prg(addr - 0x8000),
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5FFF => self.write_register(addr, value),
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr.read(self.chr_offset(addr))
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let offset = self.chr_offset(addr);
        self.chr.write(offset, value);
    }

    fn mirroring(&self) -> Mirroring {
        // Only CIRAM sources (0 and 1) can be expressed; ExRAM (2) and fill
        // (3) tables are approximated as CIRAM page 1.
        let page = |t: u8| if t == 0 { 0 } else { 1 };
        let m = [
            page(self.nametable_mapping[0]),
            page(self.nametable_mapping[1]),
            page(self.nametable_mapping[2]),
            page(self.nametable_mapping[3]),
        ];
        match m {
            [0, 1, 0, 1] => Mirroring::Vertical,
            [0, 0, 1, 1] => Mirroring::Horizontal,
            [0, 0, 0, 0] => Mirroring::SingleScreenLower,
            [1, 1, 1, 1] => Mirroring::SingleScreenUpper,
            _ => Mirroring::FourScreen,
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq_pending
    }

    fn clear_irq(&mut self) {
        self.irq_pending = false;
    }

    fn clock_scanline(&mut self) {
        if self.ppu_is_rendering {
            self.irq_in_frame = true;
            if self.scanline_counter == self.irq_scanline && self.irq_enabled {
                self.irq_pending = true;
            }
            self.scanline_counter = self.scanline_counter.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    fn mmc5() -> Mapper5 {
        // 16 x 8 KB PRG, 32 x 1 KB CHR
        Mapper5::new(
            tagged_rom(16, 0x2000),
            tagged_rom(32, 0x400),
            Mirroring::Vertical,
        )
    }

    #[test]
    fn power_on_mode_3_last_bank_everywhere() {
        let mut m = mmc5();
        assert_eq!(m.cpu_read(0xE000), 15);
        m.cpu_write(0x5114, 3);
        m.cpu_write(0x5115, 4);
        m.cpu_write(0x5116, 5);
        assert_eq!(m.cpu_read(0x8000), 3);
        assert_eq!(m.cpu_read(0xA000), 4);
        assert_eq!(m.cpu_read(0xC000), 5);
        assert_eq!(m.cpu_read(0xE000), 15);
    }

    #[test]
    fn prg_mode_0_uses_32k_bank() {
        let mut m = mmc5();
        m.cpu_write(0x5100, 0);
        m.cpu_write(0x5117, 0x0C); // bank 3 of 32 KB
        assert_eq!(m.cpu_read(0x8000), 12);
        assert_eq!(m.cpu_read(0xFFFF), 15);
    }

    #[test]
    fn chr_modes() {
        let mut m = mmc5();
        m.cpu_write(0x5101, 3); // 1 KB mode
        m.cpu_write(0x5125, 9);
        assert_eq!(m.ppu_read(0x1400), 9);
        m.cpu_write(0x5101, 0); // 8 KB mode uses $5127
        m.cpu_write(0x5127, 2);
        assert_eq!(m.ppu_read(0x0000), 16);
        assert_eq!(m.ppu_read(0x1FFF), 23);
    }

    #[test]
    fn multiplier_and_exram() {
        let mut m = mmc5();
        m.cpu_write(0x5205, 200);
        m.cpu_write(0x5206, 3);
        assert_eq!(m.cpu_read(0x5205), 0x58);
        assert_eq!(m.cpu_read(0x5206), 0x02);
        m.cpu_write(0x5C10, 0xAB);
        assert_eq!(m.cpu_read(0x5C10), 0xAB);
    }

    #[test]
    fn nametable_mapping_to_mirroring() {
        let mut m = mmc5();
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        m.cpu_write(0x5105, 0x50); // 0,0,1,1
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        m.cpu_write(0x5105, 0x00);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
    }

    #[test]
    fn scanline_irq() {
        let mut m = mmc5();
        m.cpu_write(0x5203, 2);
        m.cpu_write(0x5204, 0x80);
        m.notify_ppu_state(true);
        m.clock_scanline();
        m.clock_scanline();
        assert!(!Mapper::irq_pending(&m));
        m.clock_scanline();
        assert!(Mapper::irq_pending(&m));
        assert_eq!(m.cpu_read(0x5204) & 0x80, 0x80);
        assert!(!Mapper::irq_pending(&m));
    }
}
