//! Mapper 227 (address-latch multicart, e.g. 1200-in-1).
//!
//! The register is the CPU address of any write to $8000-$FFFF; the data
//! byte is ignored. Layout per nesdev "INES Mapper 227":
//!
//! ```text
//! [A~1... .mLQ OQQP PpMS]
//!          ||| |||| |||+- S: 0 = 16 KB mode (PRG A14 = p), 1 = 32 KB mode
//!          ||| |||| ||+-- M: 0 = vertical mirroring, 1 = horizontal
//!          ||| |||+-++--- PPp: inner 16 KB bank (PRG A16..A14)
//!          ||+-|++------- QQQ: outer 128 KB bank (PRG A19..A17)
//!          ||  +--------- O: 0 = $C000 fixed to inner bank L*7 (UNROM-like)
//!          ||               1 = NROM: $C000 mirrors $8000 (S=0) or 32 KB (S=1)
//!          |+------------ L: inner bank fixed at $C000 when O=0 (0 -> #0, 1 -> #7)
//!          +------------- m: solder-pad menu select (submapper 1 only, ignored)
//! ```
//!
//! CHR is 8 KB of unbanked RAM, write-protected while O=1 (the multicart
//! variant; the Chinese RPG boards that leave it writable are not handled).
//! Power-on register value is 0: bank 0 at both $8000 and $C000, vertical
//! mirroring, CHR RAM writable.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper227 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],
    /// Last address written to $8000-$FFFF (the address latch).
    latch: u16,
}

impl Mapper227 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, _mirroring: Mirroring) -> Self {
        Mapper227 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            latch: 0,
        }
    }

    fn s_32k(&self) -> bool {
        self.latch & 0x0001 != 0
    }

    fn o_nrom(&self) -> bool {
        self.latch & 0x0080 != 0
    }

    fn l_last(&self) -> bool {
        self.latch & 0x0200 != 0
    }

    /// Full 16 KB bank number: inner bank (bits 2-4) plus outer bank
    /// (bits 5-6 and bit 8), i.e. PRG A19..A14.
    fn bank(&self) -> usize {
        let a = self.latch as usize;
        ((a >> 2) & 0x1F) | ((a >> 3) & 0x20)
    }

    /// 16 KB bank mapped at $8000-$BFFF.
    fn low_bank(&self) -> usize {
        if self.s_32k() {
            self.bank() & !1
        } else {
            self.bank()
        }
    }

    /// 16 KB bank mapped at $C000-$FFFF.
    fn high_bank(&self) -> usize {
        if self.o_nrom() {
            if self.s_32k() {
                self.bank() | 1
            } else {
                self.bank()
            }
        } else {
            let outer = self.bank() & !7;
            if self.l_last() {
                outer | 7
            } else {
                outer
            }
        }
    }
}

impl Mapper for Mapper227 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xBFFF => {
                let offset = self.low_bank() * 0x4000 + (addr - 0x8000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            0xC000..=0xFFFF => {
                let offset = self.high_bank() * 0x4000 + (addr - 0xC000) as usize;
                prg_read(&self.prg_rom, offset)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = value,
            0x8000..=0xFFFF => self.latch = addr,
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr.read((addr & 0x1FFF) as usize)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        if !self.o_nrom() {
            self.chr.write((addr & 0x1FFF) as usize, value);
        }
    }

    fn mirroring(&self) -> Mirroring {
        if self.latch & 0x0002 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 32 banks of 16 KB (512 KB), the 1200-in-1 layout.
    fn mapper() -> Mapper227 {
        Mapper227::new(tagged_rom(32, 0x4000), vec![], Mirroring::Horizontal)
    }

    /// Register address for the given fields (bank is the 6-bit PRG A19..A14).
    fn reg(bank: u16, s: bool, m: bool, o: bool, l: bool) -> u16 {
        0x8000
            | ((bank & 0x1F) << 2)
            | ((bank & 0x20) << 3)
            | (s as u16)
            | ((m as u16) << 1)
            | ((o as u16) << 7)
            | ((l as u16) << 9)
    }

    #[test]
    fn power_on_maps_bank_zero_everywhere() {
        let mut m = mapper();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xBFFF), 0);
        assert_eq!(m.cpu_read(0xC000), 0);
        assert_eq!(m.cpu_read(0xFFFF), 0);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn unrom_mode_fixes_c000_to_inner_bank_0_or_7() {
        let mut m = mapper();
        // O=0, L=0: switchable bank at $8000, inner bank #0 of the outer
        // 128 KB block at $C000.
        m.cpu_write(reg(5, false, false, false, false), 0);
        assert_eq!(m.cpu_read(0x8000), 5);
        assert_eq!(m.cpu_read(0xC000), 0);
        // Outer block 1 (banks 8-15): inner bank #0 is bank 8.
        m.cpu_write(reg(13, false, false, false, false), 0);
        assert_eq!(m.cpu_read(0x8000), 13);
        assert_eq!(m.cpu_read(0xC000), 8);
        // O=0, L=1: inner bank #7 of the block is fixed at $C000.
        m.cpu_write(reg(13, false, false, false, true), 0);
        assert_eq!(m.cpu_read(0x8000), 13);
        assert_eq!(m.cpu_read(0xC000), 15);
        m.cpu_write(reg(2, false, false, false, true), 0);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.cpu_read(0xC000), 7);
    }

    #[test]
    fn unrom_mode_with_s_set_only_reaches_even_banks() {
        let mut m = mapper();
        m.cpu_write(reg(11, true, false, false, true), 0);
        assert_eq!(m.cpu_read(0x8000), 10);
        assert_eq!(m.cpu_read(0xC000), 15);
    }

    #[test]
    fn nrom128_mode_mirrors_the_16k_bank() {
        let mut m = mapper();
        m.cpu_write(reg(9, false, false, true, false), 0);
        assert_eq!(m.cpu_read(0x8000), 9);
        assert_eq!(m.cpu_read(0xBFFF), 9);
        assert_eq!(m.cpu_read(0xC000), 9);
        assert_eq!(m.cpu_read(0xFFFF), 9);
        // L is ignored when O=1.
        m.cpu_write(reg(9, false, false, true, true), 0);
        assert_eq!(m.cpu_read(0xC000), 9);
    }

    #[test]
    fn nrom256_mode_maps_32k_bank() {
        let mut m = mapper();
        // Bank 9 in 32 KB mode: the low bit is dropped, giving banks 8 and 9.
        m.cpu_write(reg(9, true, false, true, false), 0);
        assert_eq!(m.cpu_read(0x8000), 8);
        assert_eq!(m.cpu_read(0xC000), 9);
        m.cpu_write(reg(30, true, false, true, true), 0);
        assert_eq!(m.cpu_read(0x8000), 30);
        assert_eq!(m.cpu_read(0xFFFF), 31);
    }

    #[test]
    fn bit_8_is_the_high_bank_bit() {
        // 64 banks so bit 8 (PRG A19) selects a real bank.
        let mut m = Mapper227::new(tagged_rom(64, 0x4000), vec![], Mirroring::Horizontal);
        m.cpu_write(reg(0x23, false, false, true, false), 0);
        assert_eq!(m.cpu_read(0x8000), 0x23);
        // Bit 7 (O) sits between the two bank fields and must not leak in.
        m.cpu_write(reg(0x03, false, false, true, false), 0);
        assert_eq!(m.cpu_read(0x8000), 0x03);
        // On a 512 KB image bit 8 wraps to the ROM size.
        let mut m = mapper();
        m.cpu_write(reg(0x23, false, false, true, false), 0);
        assert_eq!(m.cpu_read(0x8000), 0x03);
    }

    #[test]
    fn latch_ignores_data_and_takes_any_address() {
        let mut m = mapper();
        // Bits 10-14 (m and the unused address lines) do not affect banking.
        m.cpu_write(reg(4, false, false, true, false) | 0x7C00, 0xFF);
        assert_eq!(m.cpu_read(0x8000), 4);
        // Writes below $8000 do not touch the latch.
        m.cpu_write(0x7FFF, 0);
        assert_eq!(m.cpu_read(0x8000), 4);
        assert_eq!(m.cpu_read(0x7FFF), 0);
    }

    #[test]
    fn mirroring_follows_bit_1() {
        let mut m = mapper();
        m.cpu_write(reg(0, false, true, false, false), 0);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        m.cpu_write(reg(0, false, false, false, false), 0);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn chr_ram_write_protected_while_o_set() {
        let mut m = mapper();
        m.ppu_write(0x0010, 0x11);
        assert_eq!(m.ppu_read(0x0010), 0x11);
        // O=1 (NROM modes): writes are ignored, reads still work.
        m.cpu_write(reg(0, false, false, true, false), 0);
        m.ppu_write(0x0010, 0x22);
        assert_eq!(m.ppu_read(0x0010), 0x11);
        m.cpu_write(reg(0, true, false, true, false), 0);
        m.ppu_write(0x0010, 0x33);
        assert_eq!(m.ppu_read(0x0010), 0x11);
        // Back to O=0: writable again.
        m.cpu_write(reg(0, false, false, false, false), 0);
        m.ppu_write(0x0010, 0x44);
        assert_eq!(m.ppu_read(0x0010), 0x44);
    }
}
