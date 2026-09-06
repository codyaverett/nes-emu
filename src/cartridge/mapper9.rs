//! Mapper 9 (MMC2 / PNROM), and the core shared with mapper 10 (MMC4).
//!
//! Used by Mike Tyson's Punch-Out.
//!
//! Registers (CPU addresses, data bits shown):
//!   $A000-$AFFF  PRG bank (bits 0-3): 8 KB at $8000-$9FFF; $A000-$FFFF is
//!                fixed to the last three 8 KB banks
//!   $B000-$BFFF  4 KB CHR bank for $0000-$0FFF while latch 0 = $FD
//!   $C000-$CFFF  4 KB CHR bank for $0000-$0FFF while latch 0 = $FE
//!   $D000-$DFFF  4 KB CHR bank for $1000-$1FFF while latch 1 = $FD
//!   $E000-$EFFF  4 KB CHR bank for $1000-$1FFF while latch 1 = $FE
//!   $F000-$FFFF  mirroring (bit 0: 0 = vertical, 1 = horizontal)
//!
//! The latches are flipped by the PPU address bus, which the PPU reports
//! through `Mapper::ppu_fetch` after each pattern read (nesdev "MMC2"):
//!   read of $0FD8            latch 0 = $FD    (MMC4: $0FD8-$0FDF)
//!   read of $0FE8            latch 0 = $FE    (MMC4: $0FE8-$0FEF)
//!   read of $1FD8-$1FDF      latch 1 = $FD
//!   read of $1FE8-$1FEF      latch 1 = $FE
//! The byte at the triggering address comes from the bank selected before
//! the flip; the next fetch uses the new bank. `ppu_peek` never flips.
//!
//! MMC4 (Fire Emblem) differs only in PRG (16 KB at $8000-$BFFF, last 16 KB
//! fixed) and in latch 0 responding to the whole $xFD8-$xFDF row range, so
//! `mapper10.rs` wraps `Mmc2Core` with `Kind::Mmc4`.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Mmc2,
    Mmc4,
}

pub(crate) struct Mmc2Core {
    kind: Kind,
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    prg_bank: u8,
    /// 4 KB CHR banks used while the table's latch is $FD, per table.
    chr_fd: [u8; 2],
    /// 4 KB CHR banks used while the table's latch is $FE, per table.
    chr_fe: [u8; 2],
    /// Per pattern table: true while the latch holds $FE.
    latch_fe: [bool; 2],
    /// 0 = vertical, 1 = horizontal.
    mirroring: u8,
}

impl Mmc2Core {
    pub fn new(kind: Kind, prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mmc2Core {
            kind,
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            prg_bank: 0,
            chr_fd: [0; 2],
            chr_fe: [0; 2],
            // Power-on latch state is not specified; $FE is the common
            // emulator choice and games set the banks before rendering.
            latch_fe: [true; 2],
            mirroring: match mirroring {
                Mirroring::Horizontal => 1,
                _ => 0,
            },
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let table = ((addr >> 12) & 1) as usize;
        let bank = if self.latch_fe[table] {
            self.chr_fe[table]
        } else {
            self.chr_fd[table]
        };
        bank as usize * 0x1000 + (addr & 0x0FFF) as usize
    }
}

impl Mapper for Mmc2Core {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match (self.kind, addr) {
            (_, 0x6000..=0x7FFF) => self.prg_ram[(addr - 0x6000) as usize],
            (Kind::Mmc2, 0x8000..=0x9FFF) => {
                let offset = self.prg_bank as usize * 0x2000 + (addr - 0x8000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            (Kind::Mmc2, 0xA000..=0xFFFF) => {
                let base = self.prg_rom.len().saturating_sub(0x6000);
                prg_read(&self.prg_rom, base + (addr - 0xA000) as usize)
            }
            (Kind::Mmc4, 0x8000..=0xBFFF) => {
                let offset = self.prg_bank as usize * 0x4000 + (addr - 0x8000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            (Kind::Mmc4, 0xC000..=0xFFFF) => {
                let base = self.prg_rom.len().saturating_sub(0x4000);
                prg_read(&self.prg_rom, base + (addr - 0xC000) as usize)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0xA000..=0xAFFF => self.prg_bank = value & 0x0F,
            0xB000..=0xBFFF => self.chr_fd[0] = value & 0x1F,
            0xC000..=0xCFFF => self.chr_fe[0] = value & 0x1F,
            0xD000..=0xDFFF => self.chr_fd[1] = value & 0x1F,
            0xE000..=0xEFFF => self.chr_fe[1] = value & 0x1F,
            0xF000..=0xFFFF => self.mirroring = value & 0x01,
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

    fn ppu_fetch(&mut self, addr: u16) {
        // MMC2 latch 0 only decodes row 0 of the tile; MMC4 latch 0 and
        // latch 1 on both chips decode all eight rows.
        let fe = match (self.kind, addr) {
            (Kind::Mmc2, 0x0FD8) => false,
            (Kind::Mmc2, 0x0FE8) => true,
            (Kind::Mmc4, 0x0FD8..=0x0FDF) => false,
            (Kind::Mmc4, 0x0FE8..=0x0FEF) => true,
            (_, 0x1FD8..=0x1FDF) => false,
            (_, 0x1FE8..=0x1FEF) => true,
            _ => return,
        };
        self.latch_fe[((addr >> 12) & 1) as usize] = fe;
    }

    fn mirroring(&self) -> Mirroring {
        if self.mirroring == 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        }
    }

    fn prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }

    // State: prg_bank u8, chr_fd 2 x u8, chr_fe 2 x u8, latch_fe 2 x bool,
    // mirroring u8, PRG RAM 8 KB, CHR.
    fn save_state(&self, w: &mut crate::state::Writer) {
        w.u8(self.prg_bank);
        w.bytes(&self.chr_fd);
        w.bytes(&self.chr_fe);
        w.bool(self.latch_fe[0]);
        w.bool(self.latch_fe[1]);
        w.u8(self.mirroring);
        w.bytes(&self.prg_ram);
        self.chr.save_state(w);
    }

    fn load_state(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.prg_bank = r.u8()?;
        r.bytes(&mut self.chr_fd)?;
        r.bytes(&mut self.chr_fe)?;
        self.latch_fe[0] = r.bool()?;
        self.latch_fe[1] = r.bool()?;
        self.mirroring = r.u8()?;
        r.bytes(&mut self.prg_ram)?;
        self.chr.load_state(r)
    }
}

pub struct Mapper9(Mmc2Core);

impl Mapper9 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper9(Mmc2Core::new(Kind::Mmc2, prg_rom, chr_rom, mirroring))
    }
}

/// Forward every `Mapper` method of a newtype around `Mmc2Core`.
macro_rules! forward_mmc2_core {
    ($name:ident) => {
        impl Mapper for $name {
            fn cpu_read(&mut self, addr: u16) -> u8 {
                self.0.cpu_read(addr)
            }
            fn cpu_write(&mut self, addr: u16, value: u8) {
                self.0.cpu_write(addr, value)
            }
            fn cpu_peek(&self, addr: u16) -> u8 {
                self.0.cpu_peek(addr)
            }
            fn ppu_read(&mut self, addr: u16) -> u8 {
                self.0.ppu_read(addr)
            }
            fn ppu_write(&mut self, addr: u16, value: u8) {
                self.0.ppu_write(addr, value)
            }
            fn ppu_peek(&self, addr: u16) -> u8 {
                self.0.ppu_peek(addr)
            }
            fn ppu_fetch(&mut self, addr: u16) {
                self.0.ppu_fetch(addr)
            }
            fn mirroring(&self) -> Mirroring {
                self.0.mirroring()
            }
            fn prg_ram(&self) -> Option<&[u8]> {
                self.0.prg_ram()
            }
            fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
                self.0.prg_ram_mut()
            }
            fn save_state(&self, w: &mut crate::state::Writer) {
                self.0.save_state(w)
            }
            fn load_state(
                &mut self,
                r: &mut crate::state::Reader,
            ) -> Result<(), crate::state::StateError> {
                self.0.load_state(r)
            }
        }
    };
}
pub(crate) use forward_mmc2_core;

forward_mmc2_core!(Mapper9);

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 16 x 8 KB PRG, 32 x 4 KB CHR (tagged 0x80 | bank).
    fn mmc2() -> Mapper9 {
        let chr: Vec<u8> = tagged_rom(32, 0x1000).iter().map(|b| b | 0x80).collect();
        Mapper9::new(tagged_rom(16, 0x2000), chr, Mirroring::Vertical)
    }

    fn set_chr(m: &mut Mapper9, fd0: u8, fe0: u8, fd1: u8, fe1: u8) {
        m.cpu_write(0xB000, fd0);
        m.cpu_write(0xC000, fe0);
        m.cpu_write(0xD000, fd1);
        m.cpu_write(0xE000, fe1);
    }

    #[test]
    fn prg_8k_switchable_with_last_three_fixed() {
        let mut m = mmc2();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xA000), 13);
        assert_eq!(m.cpu_read(0xC000), 14);
        assert_eq!(m.cpu_read(0xE000), 15);
        assert_eq!(m.cpu_read(0xFFFF), 15);
        m.cpu_write(0xA000, 7);
        assert_eq!(m.cpu_read(0x8000), 7);
        assert_eq!(m.cpu_read(0x9FFF), 7);
        assert_eq!(m.cpu_read(0xA000), 13);
        // Only bits 0-3 are the bank.
        m.cpu_write(0xAFFF, 0xF3);
        assert_eq!(m.cpu_read(0x8000), 3);
        // Writes below $A000 are not registers.
        m.cpu_write(0x8000, 9);
        assert_eq!(m.cpu_read(0x8000), 3);
    }

    #[test]
    fn prg_bank_wraps_to_rom_size() {
        let mut m = Mapper9::new(tagged_rom(4, 0x2000), vec![], Mirroring::Vertical);
        m.cpu_write(0xA000, 6);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.cpu_read(0xA000), 1);
        assert_eq!(m.cpu_read(0xE000), 3);
    }

    #[test]
    fn chr_banks_follow_each_tables_latch() {
        let mut m = mmc2();
        set_chr(&mut m, 1, 2, 3, 4);
        // Power-on latches are $FE.
        assert_eq!(m.ppu_read(0x0000), 0x82);
        assert_eq!(m.ppu_read(0x1000), 0x84);
        m.ppu_fetch(0x0FD8);
        assert_eq!(m.ppu_read(0x0000), 0x81);
        assert_eq!(m.ppu_read(0x0FFF), 0x81);
        assert_eq!(m.ppu_read(0x1000), 0x84, "latch 1 untouched");
        m.ppu_fetch(0x1FD8);
        assert_eq!(m.ppu_read(0x1000), 0x83);
        assert_eq!(m.ppu_read(0x0000), 0x81, "latch 0 untouched");
        m.ppu_fetch(0x0FE8);
        m.ppu_fetch(0x1FE8);
        assert_eq!(m.ppu_read(0x0000), 0x82);
        assert_eq!(m.ppu_read(0x1000), 0x84);
        // Registers take effect immediately for the active latch.
        m.cpu_write(0xC000, 9);
        assert_eq!(m.ppu_read(0x0000), 0x89);
        m.cpu_write(0xB000, 10);
        assert_eq!(m.ppu_read(0x0000), 0x89, "FD bank is not the active one");
    }

    #[test]
    fn latch_0_decodes_row_0_only_on_mmc2() {
        let mut m = mmc2();
        set_chr(&mut m, 1, 2, 3, 4);
        m.ppu_fetch(0x0FD8);
        assert_eq!(m.ppu_read(0x0000), 0x81);
        // Rows 1-7 of tile $FE do not flip latch 0 on the MMC2.
        for row in 1..8u16 {
            m.ppu_fetch(0x0FE8 + row);
            assert_eq!(m.ppu_read(0x0000), 0x81, "row {row}");
        }
        m.ppu_fetch(0x0FE8);
        assert_eq!(m.ppu_read(0x0000), 0x82);
        // Rows 1-7 of tile $FD do not flip either.
        for row in 1..8u16 {
            m.ppu_fetch(0x0FD8 + row);
            assert_eq!(m.ppu_read(0x0000), 0x82, "row {row}");
        }
    }

    #[test]
    fn latch_1_decodes_all_rows() {
        let mut m = mmc2();
        set_chr(&mut m, 1, 2, 3, 4);
        for row in 0..8u16 {
            m.ppu_fetch(0x1FD8 + row);
            assert_eq!(m.ppu_read(0x1000), 0x83, "row {row}");
            m.ppu_fetch(0x1FE8 + row);
            assert_eq!(m.ppu_read(0x1000), 0x84, "row {row}");
        }
    }

    #[test]
    fn other_fetches_and_low_plane_do_not_flip() {
        let mut m = mmc2();
        set_chr(&mut m, 1, 2, 3, 4);
        m.ppu_fetch(0x0FD8);
        m.ppu_fetch(0x1FD8);
        // Low plane of $FE ($xFE0-$xFE7), tile $FC, tile $FF, and
        // nametable addresses are not triggers.
        for addr in [
            0x0FE0, 0x0FE7, 0x1FE0, 0x1FE7, 0x0FC8, 0x1FF8, 0x0FF8, 0x2FE8, 0x0000,
        ] {
            m.ppu_fetch(addr);
        }
        assert_eq!(m.ppu_read(0x0000), 0x81);
        assert_eq!(m.ppu_read(0x1000), 0x83);
    }

    #[test]
    fn read_at_trigger_address_uses_old_bank_and_peek_never_flips() {
        let mut m = mmc2();
        set_chr(&mut m, 1, 2, 3, 4);
        // Latch 0 is $FE; the byte at $0FD8 itself comes from the FE bank
        // because the PPU calls ppu_read before ppu_fetch.
        assert_eq!(m.ppu_read(0x0FD8), 0x82);
        assert_eq!(m.ppu_peek(0x0FD8), 0x82);
        assert_eq!(m.ppu_read(0x0000), 0x82);
        m.ppu_fetch(0x0FD8);
        assert_eq!(m.ppu_peek(0x0000), 0x81);
        assert_eq!(m.ppu_peek(0x0000), m.ppu_read(0x0000));
    }

    #[test]
    fn chr_bank_wraps_to_image_size() {
        let mut m = Mapper9::new(
            tagged_rom(2, 0x2000),
            tagged_rom(4, 0x1000),
            Mirroring::Vertical,
        );
        m.cpu_write(0xC000, 6);
        assert_eq!(m.ppu_read(0x0000), 2);
        m.cpu_write(0xE000, 0x1F);
        assert_eq!(m.ppu_read(0x1000), 3);
    }

    #[test]
    fn mirroring_register() {
        let mut m = mmc2();
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        m.cpu_write(0xF000, 1);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        m.cpu_write(0xFFFF, 0xFE);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        let h = Mapper9::new(tagged_rom(2, 0x2000), vec![], Mirroring::Horizontal);
        assert_eq!(h.mirroring(), Mirroring::Horizontal);
    }

    #[test]
    fn prg_ram_is_readable_and_writable() {
        let mut m = mmc2();
        m.cpu_write(0x6000, 0x42);
        m.cpu_write(0x7FFF, 0x24);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        assert_eq!(m.cpu_peek(0x7FFF), 0x24);
        assert_eq!(m.prg_ram().unwrap()[0], 0x42);
    }

    #[test]
    fn save_state_round_trips_banks_and_latches() {
        use crate::state::{Reader, Writer};
        let mut m = mmc2();
        m.cpu_write(0xA000, 5);
        set_chr(&mut m, 1, 2, 3, 4);
        m.ppu_fetch(0x0FD8); // latch 0 = FD, latch 1 stays FE
        m.cpu_write(0xF000, 1);
        m.cpu_write(0x6010, 0x5A);
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();
        let snapshot = |m: &mut Mapper9| {
            (
                m.cpu_read(0x8000),
                m.ppu_read(0x0000),
                m.ppu_read(0x1000),
                m.mirroring(),
            )
        };
        let before = snapshot(&mut m);
        assert_eq!(before, (5, 0x81, 0x84, Mirroring::Horizontal));

        m.cpu_write(0xA000, 2);
        set_chr(&mut m, 9, 10, 11, 12);
        m.ppu_fetch(0x0FE8);
        m.ppu_fetch(0x1FD8);
        m.cpu_write(0xF000, 0);
        assert_ne!(snapshot(&mut m), before);

        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(snapshot(&mut m), before);
        assert_eq!(m.cpu_read(0x6010), 0x5A);
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_eq!(again.into_bytes(), bytes);
    }
}
