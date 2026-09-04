//! Mapper 0 (NROM). Also the fallback for unsupported mapper numbers.
//!
//! PRG: 16 KB (mirrored at $C000) or 32 KB. Larger images, which only occur
//! for unknown mappers falling back here, expose their last 32 KB so the
//! reset vector is reachable. CHR: 8 KB ROM, or 8 KB RAM when the header
//! reports no CHR.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper0 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    mirroring: Mirroring,
}

impl Mapper0 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper0 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            mirroring,
        }
    }

    fn prg_offset(&self, addr: u16) -> usize {
        let rel = (addr - 0x8000) as usize;
        let len = self.prg_rom.len();
        if len <= 0x8000 {
            // 16 KB mirrors via the modulo in prg_read; 32 KB maps directly.
            rel
        } else {
            // Oversized image (unknown mapper fallback): map the last 32 KB.
            len - 0x8000 + rel
        }
    }
}

impl Mapper for Mapper0 {
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
            0x8000..=0xFFFF => log::warn!("Attempting to write to ROM at {:04X}", addr),
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
    fn prg_16k_mirrors_into_upper_half() {
        let mut m = Mapper0::new(
            tagged_rom(1, 0x4000),
            tagged_rom(1, 0x2000),
            Mirroring::Vertical,
        );
        let mut prg = tagged_rom(1, 0x4000);
        prg[0x3FFC] = 0x34;
        let mut m2 = Mapper0::new(prg, vec![], Mirroring::Vertical);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 0);
        assert_eq!(m2.cpu_read(0xBFFC), 0x34);
        assert_eq!(m2.cpu_read(0xFFFC), 0x34);
    }

    #[test]
    fn prg_32k_maps_directly() {
        let mut m = Mapper0::new(tagged_rom(2, 0x4000), vec![], Mirroring::Horizontal);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 1);
        assert_eq!(m.cpu_read(0xFFFF), 1);
    }

    #[test]
    fn oversized_prg_exposes_last_32k() {
        let mut m = Mapper0::new(tagged_rom(8, 0x4000), vec![], Mirroring::Horizontal);
        assert_eq!(m.cpu_read(0x8000), 6);
        assert_eq!(m.cpu_read(0xC000), 7);
    }

    #[test]
    fn chr_rom_is_read_only_and_chr_ram_is_writable() {
        let mut rom = Mapper0::new(vec![0; 0x4000], vec![0xAB; 0x2000], Mirroring::Horizontal);
        rom.ppu_write(0x0010, 0x01);
        assert_eq!(rom.ppu_read(0x0010), 0xAB);

        let mut ram = Mapper0::new(vec![0; 0x4000], vec![], Mirroring::Horizontal);
        assert_eq!(ram.ppu_read(0x1FFF), 0);
        ram.ppu_write(0x1FFF, 0x5A);
        assert_eq!(ram.ppu_read(0x1FFF), 0x5A);
    }

    #[test]
    fn prg_ram_round_trips() {
        let mut m = Mapper0::new(vec![0; 0x4000], vec![], Mirroring::Horizontal);
        m.cpu_write(0x6123, 0x77);
        assert_eq!(m.cpu_read(0x6123), 0x77);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
    }
}
