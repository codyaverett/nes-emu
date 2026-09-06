//! Mapper 66 (GxROM: GNROM, MHROM).
//!
//! Used by Super Mario Bros / Duck Hunt, Dragon Power, Gumshoe, Doraemon.
//!
//! One register, written anywhere in $8000-$FFFF:
//!   bits 0-1  8 KB CHR bank for $0000-$1FFF
//!   bits 4-5  32 KB PRG bank for $8000-$FFFF
//! Mirroring comes from the header. Bus conflicts are not modelled.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper66 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    mirroring: Mirroring,
    prg_bank: u8,
    chr_bank: u8,
}

impl Mapper66 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper66 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            mirroring,
            prg_bank: 0,
            chr_bank: 0,
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        self.chr_bank as usize * 0x2000 + (addr & 0x1FFF) as usize
    }
}

impl Mapper for Mapper66 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => {
                let offset = self.prg_bank as usize * 0x8000 + (addr - 0x8000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000..=0xFFFF => {
                self.chr_bank = value & 0x03;
                self.prg_bank = (value >> 4) & 0x03;
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr.read(self.chr_offset(addr))
    }

    fn ppu_peek(&self, addr: u16) -> u8 {
        self.chr.read(self.chr_offset(addr))
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let offset = self.chr_offset(addr);
        self.chr.write(offset, value);
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }

    // State: mirroring u8, prg_bank u8, chr_bank u8, PRG RAM 8 KB, CHR.
    fn save_state(&self, w: &mut crate::state::Writer) {
        w.u8(super::mapper::mirroring_to_u8(self.mirroring));
        w.u8(self.prg_bank);
        w.u8(self.chr_bank);
        w.bytes(&self.prg_ram);
        self.chr.save_state(w);
    }

    fn load_state(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.mirroring = super::mapper::mirroring_from_u8(r.u8()?)?;
        self.prg_bank = r.u8()?;
        self.chr_bank = r.u8()?;
        r.bytes(&mut self.prg_ram)?;
        self.chr.load_state(r)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 4 x 32 KB PRG, 4 x 8 KB CHR (tagged 0x80 | bank).
    fn gxrom() -> Mapper66 {
        let chr: Vec<u8> = tagged_rom(4, 0x2000).iter().map(|b| b | 0x80).collect();
        Mapper66::new(tagged_rom(4, 0x8000), chr, Mirroring::Horizontal)
    }

    #[test]
    fn power_on_is_bank_0() {
        let mut m = gxrom();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xFFFF), 0);
        assert_eq!(m.ppu_read(0x0000), 0x80);
    }

    #[test]
    fn prg_bank_from_bits_4_5() {
        let mut m = gxrom();
        for bank in 0..4u8 {
            m.cpu_write(0x8000, bank << 4);
            assert_eq!(m.cpu_read(0x8000), bank);
            assert_eq!(m.cpu_read(0xBFFF), bank);
            assert_eq!(m.cpu_read(0xFFFF), bank);
        }
        // Bits 6-7 and 2-3 are not part of either field.
        m.cpu_write(0x8000, 0xCC);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.ppu_read(0x0000), 0x80);
    }

    #[test]
    fn chr_bank_from_bits_0_1() {
        let mut m = gxrom();
        for bank in 0..4u8 {
            m.cpu_write(0xFFFF, bank);
            assert_eq!(m.ppu_read(0x0000), 0x80 | bank);
            assert_eq!(m.ppu_read(0x1FFF), 0x80 | bank);
            assert_eq!(m.ppu_peek(0x1000), 0x80 | bank);
        }
    }

    #[test]
    fn prg_and_chr_fields_are_independent() {
        let mut m = gxrom();
        m.cpu_write(0x8000, 0x21);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.ppu_read(0x0000), 0x81);
    }

    #[test]
    fn banks_wrap_to_image_size() {
        let mut m = Mapper66::new(
            tagged_rom(2, 0x8000),
            tagged_rom(2, 0x2000),
            Mirroring::Horizontal,
        );
        m.cpu_write(0x8000, 0x33);
        assert_eq!(m.cpu_read(0x8000), 1);
        assert_eq!(m.ppu_read(0x0000), 1);
    }

    #[test]
    fn mirroring_comes_from_header() {
        assert_eq!(gxrom().mirroring(), Mirroring::Horizontal);
        let v = Mapper66::new(tagged_rom(1, 0x8000), vec![], Mirroring::Vertical);
        assert_eq!(v.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn save_state_round_trips_banks() {
        use crate::state::{Reader, Writer};
        let mut m = gxrom();
        m.cpu_write(0x8000, 0x32);
        m.cpu_write(0x7FFF, 0x44);
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();

        m.cpu_write(0x8000, 0x11);
        assert_eq!(m.cpu_read(0x8000), 1);
        assert_eq!(m.ppu_read(0x0000), 0x81);

        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(m.cpu_read(0x8000), 3);
        assert_eq!(m.ppu_read(0x0000), 0x82);
        assert_eq!(m.cpu_read(0x7FFF), 0x44);
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_eq!(again.into_bytes(), bytes);
    }
}
