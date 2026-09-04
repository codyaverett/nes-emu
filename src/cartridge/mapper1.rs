//! Mapper 1 (MMC1 / SxROM).
//!
//! Registers are loaded one bit at a time through a 5-bit shift register at
//! $8000-$FFFF; the fifth write commits to the register selected by address
//! bits 13-14. Control ($8000): mirroring (bits 0-1), PRG mode (2-3), CHR mode
//! (4). CHR bank 0 ($A000), CHR bank 1 ($C000), PRG bank ($E000).

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper1 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],

    shift_register: u8,
    shift_count: u8,
    control: u8,
    chr_bank_0: u8,
    chr_bank_1: u8,
    prg_bank: u8,
}

impl Mapper1 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, header_mirroring: Mirroring) -> Self {
        // Power-on: 16 KB PRG mode with the last bank fixed at $C000 (mode 3).
        // Seed the mirroring bits from the header so nothing changes until
        // the game writes the control register.
        let mirror_bits = match header_mirroring {
            Mirroring::Vertical => 0x02,
            Mirroring::Horizontal => 0x03,
            Mirroring::SingleScreenLower => 0x00,
            Mirroring::SingleScreenUpper => 0x01,
            Mirroring::FourScreen => 0x03,
        };
        Mapper1 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            shift_register: 0,
            shift_count: 0,
            control: 0x0C | mirror_bits,
            chr_bank_0: 0,
            chr_bank_1: 0,
            prg_bank: 0,
        }
    }

    fn prg_mode(&self) -> u8 {
        (self.control >> 2) & 0x03
    }

    fn chr_4k_mode(&self) -> bool {
        self.control & 0x10 != 0
    }

    fn prg_offset(&self, addr: u16) -> usize {
        let rel = (addr - 0x8000) as usize;
        let prg_banks = (self.prg_rom.len() / 0x4000).max(1);
        match self.prg_mode() {
            0 | 1 => {
                // 32 KB mode: ignore the low bit of the bank number
                (self.prg_bank & 0xFE) as usize * 0x4000 + rel
            }
            2 => {
                // First bank fixed at $8000, switchable bank at $C000
                if rel < 0x4000 {
                    rel
                } else {
                    self.prg_bank as usize * 0x4000 + (rel - 0x4000)
                }
            }
            _ => {
                // Switchable bank at $8000, last bank fixed at $C000
                if rel < 0x4000 {
                    self.prg_bank as usize * 0x4000 + rel
                } else {
                    (prg_banks - 1) * 0x4000 + (rel - 0x4000)
                }
            }
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let addr = (addr & 0x1FFF) as usize;
        if self.chr_4k_mode() {
            if addr < 0x1000 {
                self.chr_bank_0 as usize * 0x1000 + addr
            } else {
                self.chr_bank_1 as usize * 0x1000 + (addr - 0x1000)
            }
        } else {
            (self.chr_bank_0 & 0x1E) as usize * 0x1000 + addr
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr & 0x6000 {
            0x0000 => self.control = value,
            0x2000 => self.chr_bank_0 = value,
            0x4000 => self.chr_bank_1 = value,
            _ => self.prg_bank = value & 0x0F,
        }
    }
}

impl Mapper for Mapper1 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => prg_read(&self.prg_rom, self.prg_offset(addr)),
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000..=0xFFFF => {
                if value & 0x80 != 0 {
                    // Reset: clear the shift register and force PRG mode 3
                    self.shift_register = 0;
                    self.shift_count = 0;
                    self.control |= 0x0C;
                } else {
                    self.shift_register = (self.shift_register >> 1) | ((value & 1) << 4);
                    self.shift_count += 1;
                    if self.shift_count == 5 {
                        let data = self.shift_register;
                        self.write_register(addr, data);
                        self.shift_register = 0;
                        self.shift_count = 0;
                    }
                }
            }
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
        match self.control & 0x03 {
            0 => Mirroring::SingleScreenLower,
            1 => Mirroring::SingleScreenUpper,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// Serially load a 5-bit value into the MMC1 register at `addr`.
    fn load(m: &mut Mapper1, addr: u16, value: u8) {
        for i in 0..5 {
            m.cpu_write(addr, (value >> i) & 1);
        }
    }

    fn mmc1() -> Mapper1 {
        // 8 x 16 KB PRG, 8 x 4 KB CHR
        Mapper1::new(
            tagged_rom(8, 0x4000),
            tagged_rom(8, 0x1000),
            Mirroring::Horizontal,
        )
    }

    #[test]
    fn power_on_state_is_mode_3_with_header_mirroring() {
        let mut m = mmc1();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 7);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        let v = Mapper1::new(tagged_rom(2, 0x4000), vec![], Mirroring::Vertical);
        assert_eq!(v.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn shift_register_commits_on_fifth_write() {
        let mut m = mmc1();
        // Four writes: nothing committed yet
        for _ in 0..4 {
            m.cpu_write(0xE000, 1);
        }
        assert_eq!(m.cpu_read(0x8000), 0);
        m.cpu_write(0xE000, 0); // value = 0b01111 = 15 -> wraps to bank 7
        assert_eq!(m.cpu_read(0x8000), 7);
    }

    #[test]
    fn reset_bit_clears_shift_and_forces_mode_3() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x00); // 32 KB mode, single screen lower
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
        m.cpu_write(0xE000, 1);
        m.cpu_write(0xE000, 0x80);
        assert_eq!(m.prg_mode(), 3);
        assert_eq!(m.shift_count, 0);
    }

    #[test]
    fn prg_mode_3_switches_low_bank_and_fixes_last() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x0C);
        load(&mut m, 0xE000, 3);
        assert_eq!(m.cpu_read(0x8000), 3);
        assert_eq!(m.cpu_read(0xBFFF), 3);
        assert_eq!(m.cpu_read(0xC000), 7);
    }

    #[test]
    fn prg_mode_2_fixes_first_bank_and_switches_high() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x08);
        load(&mut m, 0xE000, 5);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 5);
    }

    #[test]
    fn prg_32k_mode_ignores_low_bank_bit() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x00);
        load(&mut m, 0xE000, 5);
        assert_eq!(m.cpu_read(0x8000), 4);
        assert_eq!(m.cpu_read(0xC000), 5);
        load(&mut m, 0x8000, 0x04); // mode 1 behaves like mode 0
        assert_eq!(m.cpu_read(0x8000), 4);
    }

    #[test]
    fn chr_8k_mode_uses_even_bank_pair() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x0C); // CHR 8 KB mode
        load(&mut m, 0xA000, 5); // low bit ignored -> banks 4,5
        assert_eq!(m.ppu_read(0x0000), 4);
        assert_eq!(m.ppu_read(0x1000), 5);
        load(&mut m, 0xC000, 2); // ignored in 8 KB mode
        assert_eq!(m.ppu_read(0x1000), 5);
    }

    #[test]
    fn chr_4k_mode_switches_halves_independently() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x1C); // CHR 4 KB mode
        load(&mut m, 0xA000, 6);
        load(&mut m, 0xC000, 1);
        assert_eq!(m.ppu_read(0x0000), 6);
        assert_eq!(m.ppu_read(0x0FFF), 6);
        assert_eq!(m.ppu_read(0x1000), 1);
        assert_eq!(m.ppu_read(0x1FFF), 1);
    }

    #[test]
    fn mirroring_follows_control_register() {
        let mut m = mmc1();
        load(&mut m, 0x8000, 0x0E);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        load(&mut m, 0x8000, 0x0F);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        load(&mut m, 0x8000, 0x0D);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenUpper);
    }

    #[test]
    fn chr_ram_board_writes_through_bank() {
        let mut m = Mapper1::new(tagged_rom(2, 0x4000), vec![], Mirroring::Horizontal);
        m.ppu_write(0x0123, 0x99);
        assert_eq!(m.ppu_read(0x0123), 0x99);
        assert_eq!(m.ppu_read(0x1123), 0);
    }
}
