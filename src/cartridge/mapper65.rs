//! Mapper 65 (Irem H3001).
//!
//! PRG: three switchable 8 KB banks at $8000 ($8000), $A000 ($A000) and
//! $C000 ($C000); $E000-$FFFF fixed to the last bank.
//! CHR: eight switchable 1 KB banks at $B000-$B007.
//! Mirroring select ($9001) and the IRQ counter ($9003-$9006) are not
//! implemented (they were not before this refactor either).

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper65 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    mirroring: Mirroring,
    prg_banks: [u8; 3],
    chr_banks: [u8; 8],
}

impl Mapper65 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper65 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            mirroring,
            prg_banks: [0, 1, 2],
            chr_banks: [0, 1, 2, 3, 4, 5, 6, 7],
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let slot = ((addr & 0x1FFF) / 0x400) as usize;
        self.chr_banks[slot] as usize * 0x400 + (addr & 0x3FF) as usize
    }
}

impl Mapper for Mapper65 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xDFFF => {
                let slot = ((addr - 0x8000) / 0x2000) as usize;
                let offset = self.prg_banks[slot] as usize * 0x2000 + (addr & 0x1FFF) as usize;
                prg_read(&self.prg_rom, offset)
            }
            0xE000..=0xFFFF => {
                let last = self.prg_rom.len().saturating_sub(0x2000);
                prg_read(&self.prg_rom, last + (addr & 0x1FFF) as usize)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000 => self.prg_banks[0] = value,
            0xA000 => self.prg_banks[1] = value,
            0xC000 => self.prg_banks[2] = value,
            0xB000..=0xB007 => self.chr_banks[(addr - 0xB000) as usize] = value,
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
        self.mirroring
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    #[test]
    fn prg_banks_and_fixed_last() {
        let mut m = Mapper65::new(
            tagged_rom(16, 0x2000),
            tagged_rom(32, 0x400),
            Mirroring::Vertical,
        );
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xA000), 1);
        assert_eq!(m.cpu_read(0xC000), 2);
        assert_eq!(m.cpu_read(0xE000), 15);
        m.cpu_write(0x8000, 7);
        m.cpu_write(0xA000, 8);
        m.cpu_write(0xC000, 9);
        assert_eq!(m.cpu_read(0x9FFF), 7);
        assert_eq!(m.cpu_read(0xBFFF), 8);
        assert_eq!(m.cpu_read(0xDFFF), 9);
        assert_eq!(m.cpu_read(0xFFFF), 15);
    }

    #[test]
    fn chr_1k_banks() {
        let mut m = Mapper65::new(
            tagged_rom(4, 0x2000),
            tagged_rom(32, 0x400),
            Mirroring::Vertical,
        );
        for slot in 0..8u16 {
            assert_eq!(m.ppu_read(slot * 0x400), slot as u8);
        }
        m.cpu_write(0xB003, 20);
        assert_eq!(m.ppu_read(0x0C00), 20);
        assert_eq!(m.ppu_read(0x0FFF), 20);
        assert_eq!(m.ppu_read(0x1000), 4);
    }
}
