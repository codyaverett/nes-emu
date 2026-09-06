//! Mapper 3 (CNROM).
//!
//! PRG is NROM-style (16 KB mirrored or 32 KB). Writes to $8000-$FFFF select
//! an 8 KB CHR ROM bank (low 2 bits on real CNROM; larger values wrap).

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper3 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    mirroring: Mirroring,
    chr_bank: u8,
}

impl Mapper3 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper3 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            mirroring,
            chr_bank: 0,
        }
    }
}

impl Mapper for Mapper3 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => prg_read(&self.prg_rom, (addr - 0x8000) as usize),
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000..=0xFFFF => self.chr_bank = value & 0x03,
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let offset = self.chr_bank as usize * 0x2000 + (addr & 0x1FFF) as usize;
        self.chr.read(offset)
    }

    fn ppu_peek(&self, addr: u16) -> u8 {
        let offset = self.chr_bank as usize * 0x2000 + (addr & 0x1FFF) as usize;
        self.chr.read(offset)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let offset = self.chr_bank as usize * 0x2000 + (addr & 0x1FFF) as usize;
        self.chr.write(offset, value);
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    // State: mirroring u8, chr_bank u8, PRG RAM 8 KB, CHR.
    fn save_state(&self, w: &mut crate::state::Writer) {
        w.u8(super::mapper::mirroring_to_u8(self.mirroring));
        w.u8(self.chr_bank);
        w.bytes(&self.prg_ram);
        self.chr.save_state(w);
    }

    fn load_state(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.mirroring = super::mapper::mirroring_from_u8(r.u8()?)?;
        self.chr_bank = r.u8()?;
        r.bytes(&mut self.prg_ram)?;
        self.chr.load_state(r)
    }

    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    #[test]
    fn chr_bank_select() {
        let mut m = Mapper3::new(
            tagged_rom(2, 0x4000),
            tagged_rom(4, 0x2000),
            Mirroring::Vertical,
        );
        assert_eq!(m.ppu_read(0x0000), 0);
        assert_eq!(m.ppu_read(0x1FFF), 0);
        m.cpu_write(0x8000, 2);
        assert_eq!(m.ppu_read(0x0000), 2);
        assert_eq!(m.ppu_read(0x1FFF), 2);
        m.cpu_write(0xFFFF, 3);
        assert_eq!(m.ppu_read(0x0800), 3);
        // CHR ROM: writes are ignored
        m.ppu_write(0x0800, 0x11);
        assert_eq!(m.ppu_read(0x0800), 3);
    }

    #[test]
    fn prg_is_nrom_style() {
        let mut m = Mapper3::new(
            tagged_rom(1, 0x4000),
            tagged_rom(2, 0x2000),
            Mirroring::Vertical,
        );
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 0);
        let mut m32 = Mapper3::new(
            tagged_rom(2, 0x4000),
            tagged_rom(2, 0x2000),
            Mirroring::Vertical,
        );
        assert_eq!(m32.cpu_read(0xC000), 1);
    }

    #[test]
    fn ppu_peek_matches_ppu_read() {
        let mut m = Mapper3::new(
            tagged_rom(2, 0x4000),
            tagged_rom(4, 0x2000),
            Mirroring::Vertical,
        );
        m.cpu_write(0x8000, 2);
        for addr in (0x0000..0x2000).step_by(0x3F) {
            assert_eq!(m.ppu_peek(addr), m.ppu_read(addr), "addr {addr:04X}");
        }
        assert_eq!(m.ppu_peek(0x0000), 2);
    }

    #[test]
    fn save_state_round_trips_chr_bank() {
        use crate::state::{Reader, Writer};
        let mut m = Mapper3::new(
            tagged_rom(2, 0x4000),
            tagged_rom(4, 0x2000),
            Mirroring::Vertical,
        );
        m.cpu_write(0x8000, 3);
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();

        m.cpu_write(0x8000, 1);
        assert_eq!(m.ppu_read(0x0000), 1);
        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(m.ppu_read(0x0000), 3);
        assert_eq!(m.ppu_read(0x1FFF), 3);
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_eq!(again.into_bytes(), bytes);
    }
}
