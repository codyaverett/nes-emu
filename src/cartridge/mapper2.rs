//! Mapper 2 (UxROM).
//!
//! $8000-$BFFF: switchable 16 KB PRG bank (written to $8000-$FFFF).
//! $C000-$FFFF: fixed to the last 16 KB bank. CHR is 8 KB RAM on real boards.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper2 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    mirroring: Mirroring,
    prg_bank: u8,
}

impl Mapper2 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper2 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            mirroring,
            prg_bank: 0,
        }
    }

    fn last_bank_offset(&self) -> usize {
        self.prg_rom.len().saturating_sub(0x4000)
    }
}

impl Mapper for Mapper2 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xBFFF => {
                let offset = self.prg_bank as usize * 0x4000 + (addr - 0x8000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            0xC000..=0xFFFF => {
                let offset = self.last_bank_offset() + (addr - 0xC000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000..=0xFFFF => self.prg_bank = value & 0x0F,
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr.read((addr & 0x1FFF) as usize)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        self.chr.write((addr & 0x1FFF) as usize, value);
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    #[test]
    fn switchable_low_bank_and_fixed_high_bank() {
        let mut m = Mapper2::new(tagged_rom(8, 0x4000), vec![], Mirroring::Vertical);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 7);
        m.cpu_write(0x8000, 5);
        assert_eq!(m.cpu_read(0x8000), 5);
        assert_eq!(m.cpu_read(0xBFFF), 5);
        assert_eq!(m.cpu_read(0xFFFF), 7);
    }

    #[test]
    fn bank_wraps_to_rom_size() {
        let mut m = Mapper2::new(tagged_rom(4, 0x4000), vec![], Mirroring::Vertical);
        m.cpu_write(0xFFFF, 6);
        assert_eq!(m.cpu_read(0x8000), 2);
    }

    #[test]
    fn chr_ram_is_writable() {
        let mut m = Mapper2::new(tagged_rom(2, 0x4000), vec![], Mirroring::Vertical);
        m.ppu_write(0x0000, 0x42);
        assert_eq!(m.ppu_read(0x0000), 0x42);
    }
}
