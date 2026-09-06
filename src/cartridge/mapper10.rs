//! Mapper 10 (MMC4 / FxROM).
//!
//! Used by Fire Emblem, Fire Emblem Gaiden, Famicom Wars.
//!
//! Same registers and CHR latches as the MMC2 (see `mapper9.rs`) with two
//! differences:
//!   $A000-$AFFF  PRG bank (bits 0-3): 16 KB at $8000-$BFFF; $C000-$FFFF is
//!                fixed to the last 16 KB bank
//!   latch 0      flips on any of $0FD8-$0FDF / $0FE8-$0FEF, not just the
//!                first row, matching latch 1
//! 8 KB PRG RAM at $6000-$7FFF (battery backed on the Fire Emblem boards).

use super::mapper::Mapper;
use super::mapper9::{forward_mmc2_core, Kind, Mmc2Core};
use super::Mirroring;

pub struct Mapper10(Mmc2Core);

impl Mapper10 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper10(Mmc2Core::new(Kind::Mmc4, prg_rom, chr_rom, mirroring))
    }
}

forward_mmc2_core!(Mapper10);

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 8 x 16 KB PRG, 32 x 4 KB CHR (tagged 0x80 | bank).
    fn mmc4() -> Mapper10 {
        let chr: Vec<u8> = tagged_rom(32, 0x1000).iter().map(|b| b | 0x80).collect();
        Mapper10::new(tagged_rom(8, 0x4000), chr, Mirroring::Vertical)
    }

    fn set_chr(m: &mut Mapper10, fd0: u8, fe0: u8, fd1: u8, fe1: u8) {
        m.cpu_write(0xB000, fd0);
        m.cpu_write(0xC000, fe0);
        m.cpu_write(0xD000, fd1);
        m.cpu_write(0xE000, fe1);
    }

    #[test]
    fn prg_16k_switchable_with_last_fixed() {
        let mut m = mmc4();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xBFFF), 0);
        assert_eq!(m.cpu_read(0xC000), 7);
        assert_eq!(m.cpu_read(0xFFFF), 7);
        m.cpu_write(0xA000, 5);
        assert_eq!(m.cpu_read(0x8000), 5);
        assert_eq!(m.cpu_read(0xBFFF), 5);
        assert_eq!(m.cpu_read(0xC000), 7);
        m.cpu_write(0xAFFF, 0xF3);
        assert_eq!(m.cpu_read(0x8000), 3);
        m.cpu_write(0x9000, 1);
        assert_eq!(m.cpu_read(0x8000), 3, "sub-$A000 writes are not registers");
    }

    #[test]
    fn prg_bank_wraps_to_rom_size() {
        let mut m = Mapper10::new(tagged_rom(2, 0x4000), vec![], Mirroring::Vertical);
        m.cpu_write(0xA000, 3);
        assert_eq!(m.cpu_read(0x8000), 1);
        assert_eq!(m.cpu_read(0xC000), 1);
    }

    #[test]
    fn latch_0_decodes_all_rows_on_mmc4() {
        let mut m = mmc4();
        set_chr(&mut m, 1, 2, 3, 4);
        for row in 0..8u16 {
            m.ppu_fetch(0x0FD8 + row);
            assert_eq!(m.ppu_read(0x0000), 0x81, "row {row}");
            m.ppu_fetch(0x0FE8 + row);
            assert_eq!(m.ppu_read(0x0000), 0x82, "row {row}");
        }
    }

    #[test]
    fn latch_1_decodes_all_rows() {
        let mut m = mmc4();
        set_chr(&mut m, 1, 2, 3, 4);
        for row in 0..8u16 {
            m.ppu_fetch(0x1FD8 + row);
            assert_eq!(m.ppu_read(0x1000), 0x83, "row {row}");
            m.ppu_fetch(0x1FE8 + row);
            assert_eq!(m.ppu_read(0x1000), 0x84, "row {row}");
        }
    }

    #[test]
    fn latches_are_independent_and_other_fetches_do_not_flip() {
        let mut m = mmc4();
        set_chr(&mut m, 1, 2, 3, 4);
        m.ppu_fetch(0x0FDA);
        assert_eq!(m.ppu_read(0x0000), 0x81);
        assert_eq!(m.ppu_read(0x1000), 0x84);
        for addr in [0x0FE0, 0x0FE7, 0x0FC8, 0x0FF8, 0x1FF8, 0x2FE8] {
            m.ppu_fetch(addr);
        }
        assert_eq!(m.ppu_read(0x0000), 0x81);
        assert_eq!(m.ppu_read(0x1000), 0x84);
        assert_eq!(m.ppu_peek(0x0000), 0x81);
    }

    #[test]
    fn mirroring_register_and_prg_ram() {
        let mut m = mmc4();
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        m.cpu_write(0xF000, 1);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        m.cpu_write(0x6000, 0x42);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        assert_eq!(m.prg_ram().unwrap()[0], 0x42);
        m.prg_ram_mut().unwrap()[1] = 0x24;
        assert_eq!(m.cpu_peek(0x6001), 0x24);
    }

    #[test]
    fn save_state_round_trips_banks_and_latches() {
        use crate::state::{Reader, Writer};
        let mut m = mmc4();
        m.cpu_write(0xA000, 6);
        set_chr(&mut m, 1, 2, 3, 4);
        m.ppu_fetch(0x1FDF); // latch 1 = FD, latch 0 stays FE
        m.cpu_write(0x7000, 0xAB);
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();
        let snapshot =
            |m: &mut Mapper10| (m.cpu_read(0x8000), m.ppu_read(0x0000), m.ppu_read(0x1000));
        let before = snapshot(&mut m);
        assert_eq!(before, (6, 0x82, 0x83));

        m.cpu_write(0xA000, 1);
        set_chr(&mut m, 9, 10, 11, 12);
        m.ppu_fetch(0x0FD8);
        m.ppu_fetch(0x1FE8);
        assert_ne!(snapshot(&mut m), before);

        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(snapshot(&mut m), before);
        assert_eq!(m.cpu_read(0x7000), 0xAB);
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_eq!(again.into_bytes(), bytes);
    }
}
