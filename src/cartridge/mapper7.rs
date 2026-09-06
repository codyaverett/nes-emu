//! Mapper 7 (AxROM: ANROM, AMROM, AOROM).
//!
//! Used by Battletoads, Marble Madness, Wizards and Warriors, Cobra Triangle.
//!
//! One register, written anywhere in $8000-$FFFF:
//!   bits 0-2  32 KB PRG bank for $8000-$FFFF
//!   bit 4     single-screen nametable (0 = lower 1 KB page, 1 = upper)
//! CHR is 8 KB RAM, not banked. The header mirroring bits are ignored
//! because the register owns the nametable selection. Power-on is bank 0,
//! lower page.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper7 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    prg_bank: u8,
    upper_nametable: bool,
}

impl Mapper7 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, _mirroring: Mirroring) -> Self {
        Mapper7 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            prg_bank: 0,
            upper_nametable: false,
        }
    }
}

impl Mapper for Mapper7 {
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
                self.prg_bank = value & 0x07;
                self.upper_nametable = value & 0x10 != 0;
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr.read((addr & 0x1FFF) as usize)
    }

    fn ppu_peek(&self, addr: u16) -> u8 {
        self.chr.read((addr & 0x1FFF) as usize)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        self.chr.write((addr & 0x1FFF) as usize, value);
    }

    fn mirroring(&self) -> Mirroring {
        if self.upper_nametable {
            Mirroring::SingleScreenUpper
        } else {
            Mirroring::SingleScreenLower
        }
    }

    fn prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }

    // State: prg_bank u8, upper_nametable bool, PRG RAM 8 KB, CHR.
    fn save_state(&self, w: &mut crate::state::Writer) {
        w.u8(self.prg_bank);
        w.bool(self.upper_nametable);
        w.bytes(&self.prg_ram);
        self.chr.save_state(w);
    }

    fn load_state(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.prg_bank = r.u8()?;
        self.upper_nametable = r.bool()?;
        r.bytes(&mut self.prg_ram)?;
        self.chr.load_state(r)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    fn axrom() -> Mapper7 {
        Mapper7::new(tagged_rom(8, 0x8000), vec![], Mirroring::Horizontal)
    }

    #[test]
    fn power_on_is_bank_0_lower_nametable() {
        let mut m = axrom();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xFFFF), 0);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
    }

    #[test]
    fn prg_bank_from_bits_0_to_2_covers_32k() {
        let mut m = axrom();
        for bank in 0..8u8 {
            m.cpu_write(0x8000, bank);
            assert_eq!(m.cpu_read(0x8000), bank);
            assert_eq!(m.cpu_read(0xBFFF), bank);
            assert_eq!(m.cpu_read(0xC000), bank);
            assert_eq!(m.cpu_read(0xFFFF), bank);
        }
        // Bit 3 and above do not reach the bank number.
        m.cpu_write(0xFFFF, 0xE9);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn bank_wraps_to_rom_size() {
        let mut m = Mapper7::new(tagged_rom(2, 0x8000), vec![], Mirroring::Vertical);
        m.cpu_write(0x8000, 5);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn bit_4_selects_single_screen_page() {
        let mut m = axrom();
        m.cpu_write(0x8000, 0x10);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenUpper);
        m.cpu_write(0x8000, 0x07);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
        // Bit 3 is not the nametable bit.
        m.cpu_write(0x8000, 0x08);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
    }

    #[test]
    fn chr_ram_is_writable_and_unbanked() {
        let mut m = axrom();
        m.ppu_write(0x1FFF, 0x42);
        assert_eq!(m.ppu_read(0x1FFF), 0x42);
        assert_eq!(m.ppu_peek(0x1FFF), 0x42);
        m.cpu_write(0x8000, 3);
        assert_eq!(m.ppu_read(0x1FFF), 0x42);
    }

    #[test]
    fn save_state_round_trips_bank_page_and_chr_ram() {
        use crate::state::{Reader, Writer};
        let mut m = axrom();
        m.cpu_write(0x8000, 0x15);
        m.ppu_write(0x0123, 0x99);
        m.cpu_write(0x6000, 0x77);
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();

        m.cpu_write(0x8000, 0x02);
        m.ppu_write(0x0123, 0);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);

        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(m.cpu_read(0x8000), 5);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenUpper);
        assert_eq!(m.ppu_read(0x0123), 0x99);
        assert_eq!(m.cpu_read(0x6000), 0x77);
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_eq!(again.into_bytes(), bytes);
    }
}
