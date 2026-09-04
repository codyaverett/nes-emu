//! Mapper 4 (MMC3 / TxROM).
//!
//! Used by Super Mario Bros 2 and 3, Mega Man 3-6, TMNT, Kirby's Adventure.
//!
//! Registers (CPU addresses; even/odd selects the register within each pair):
//!   $8000 bank select, $8001 bank data,
//!   $A000 mirroring,   $A001 PRG RAM protect,
//!   $C000 IRQ latch,   $C001 IRQ reload,
//!   $E000 IRQ disable, $E001 IRQ enable.
//!
//! `clock_scanline` implements the scanline counter. Nothing calls it yet;
//! wiring it to the PPU A12 edge and delivering the IRQ is a follow-up.

use super::mapper::{Chr, Mapper};
use super::Mirroring;

pub struct Mapper4 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],

    // Bank registers
    bank_select: u8,
    bank_data: [u8; 8],

    // PRG RAM control ($A001): bit 7 enable, bit 6 write protect.
    // Hardware powers up undefined; enabled by default so games that never
    // write $A001 keep working (matches the pre-trait System behaviour).
    prg_ram_enabled: bool,
    prg_ram_write_protect: bool,

    // Mirroring
    four_screen: bool,
    mirroring: u8,

    // IRQ
    irq_enabled: bool,
    irq_counter: u8,
    irq_latch: u8,
    irq_reload: bool,
    irq_pending: bool,

    // Resolved PRG bank offsets for $8000, $A000, $C000, $E000
    prg_banks: [usize; 4],

    // Resolved CHR bank offsets for each 1 KB slot
    chr_banks: [usize; 8],
}

impl Mapper4 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, header_mirroring: Mirroring) -> Self {
        let mut mapper = Self {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            bank_select: 0,
            bank_data: [0; 8],
            prg_ram_enabled: true,
            prg_ram_write_protect: false,
            four_screen: matches!(header_mirroring, Mirroring::FourScreen),
            mirroring: match header_mirroring {
                Mirroring::Horizontal => 1,
                _ => 0,
            },
            irq_enabled: false,
            irq_counter: 0,
            irq_latch: 0,
            irq_reload: false,
            irq_pending: false,
            prg_banks: [0; 4],
            chr_banks: [0; 8],
        };
        // Bank registers power up cleared; derive the slot offsets from them.
        mapper.update_banks();
        mapper
    }

    fn update_banks(&mut self) {
        let prg_mode = (self.bank_select >> 6) & 0x01;
        let chr_mode = (self.bank_select >> 7) & 0x01;

        let prg_len = self.prg_rom.len().max(1);
        let r6 = (self.bank_data[6] as usize) * 0x2000 % prg_len;
        let r7 = (self.bank_data[7] as usize) * 0x2000 % prg_len;
        let second_last = self.prg_rom.len().saturating_sub(0x4000);
        let last = self.prg_rom.len().saturating_sub(0x2000);

        if prg_mode == 0 {
            self.prg_banks = [r6, r7, second_last, last];
        } else {
            self.prg_banks = [second_last, r7, r6, last];
        }

        let chr_len = self.chr.data.len();
        let bank = |value: u8| (value as usize) * 0x400 % chr_len;
        let r0 = self.bank_data[0];
        let r1 = self.bank_data[1];
        let r2 = self.bank_data[2];
        let r3 = self.bank_data[3];
        let r4 = self.bank_data[4];
        let r5 = self.bank_data[5];

        if chr_mode == 0 {
            // Two 2 KB banks at $0000-$0FFF, four 1 KB banks at $1000-$1FFF
            self.chr_banks = [
                bank(r0 & 0xFE),
                bank(r0 | 0x01),
                bank(r1 & 0xFE),
                bank(r1 | 0x01),
                bank(r2),
                bank(r3),
                bank(r4),
                bank(r5),
            ];
        } else {
            // Four 1 KB banks at $0000-$0FFF, two 2 KB banks at $1000-$1FFF
            self.chr_banks = [
                bank(r2),
                bank(r3),
                bank(r4),
                bank(r5),
                bank(r0 & 0xFE),
                bank(r0 | 0x01),
                bank(r1 & 0xFE),
                bank(r1 | 0x01),
            ];
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let slot = ((addr & 0x1FFF) / 0x400) as usize;
        self.chr_banks[slot] + (addr & 0x3FF) as usize
    }
}

impl Mapper for Mapper4 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    self.prg_ram[(addr & 0x1FFF) as usize]
                } else {
                    0
                }
            }
            0x8000..=0xFFFF => {
                if self.prg_rom.is_empty() {
                    return 0;
                }
                let slot = ((addr - 0x8000) / 0x2000) as usize;
                let offset = self.prg_banks[slot] + (addr & 0x1FFF) as usize;
                self.prg_rom[offset % self.prg_rom.len()]
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled && !self.prg_ram_write_protect {
                    self.prg_ram[(addr & 0x1FFF) as usize] = value;
                }
            }
            0x8000..=0x9FFF if addr & 0x01 == 0 => {
                // Bank select
                self.bank_select = value;
                self.update_banks();
            }
            0x8000..=0x9FFF => {
                // Bank data
                let bank = self.bank_select & 0x07;
                self.bank_data[bank as usize] = value;
                self.update_banks();
            }
            0xA000..=0xBFFF if addr & 0x01 == 0 => {
                // Mirroring: 0 = vertical, 1 = horizontal
                self.mirroring = value & 0x01;
            }
            0xA000..=0xBFFF => {
                // PRG RAM protect
                self.prg_ram_enabled = (value & 0x80) != 0;
                self.prg_ram_write_protect = (value & 0x40) != 0;
            }
            0xC000..=0xDFFF if addr & 0x01 == 0 => {
                // IRQ latch
                self.irq_latch = value;
            }
            0xC000..=0xDFFF => {
                // IRQ reload
                self.irq_reload = true;
                self.irq_counter = 0;
            }
            0xE000..=0xFFFF if addr & 0x01 == 0 => {
                // IRQ disable (also acknowledges)
                self.irq_enabled = false;
                self.irq_pending = false;
            }
            0xE000..=0xFFFF => {
                // IRQ enable
                self.irq_enabled = true;
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
        if self.four_screen {
            Mirroring::FourScreen
        } else if self.mirroring == 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq_pending
    }

    fn clear_irq(&mut self) {
        self.irq_pending = false;
    }

    fn clock_scanline(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter -= 1;
        }

        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_pending = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 16 x 8 KB PRG banks, 64 x 1 KB CHR banks.
    fn mmc3() -> Mapper4 {
        Mapper4::new(
            tagged_rom(16, 0x2000),
            tagged_rom(64, 0x400),
            Mirroring::Vertical,
        )
    }

    fn set_bank(m: &mut Mapper4, mode_bits: u8, reg: u8, value: u8) {
        m.cpu_write(0x8000, mode_bits | reg);
        m.cpu_write(0x8001, value);
    }

    #[test]
    fn power_on_prg_layout() {
        let mut m = mmc3();
        // R6 and R7 power up at 0; the upper two slots are fixed
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xA000), 0);
        assert_eq!(m.cpu_read(0xC000), 14);
        assert_eq!(m.cpu_read(0xE000), 15);
        assert_eq!(m.cpu_read(0xFFFF), 15);
    }

    #[test]
    fn prg_mode_0_switches_8000_and_a000() {
        let mut m = mmc3();
        set_bank(&mut m, 0x00, 6, 9);
        set_bank(&mut m, 0x00, 7, 3);
        assert_eq!(m.cpu_read(0x8000), 9);
        assert_eq!(m.cpu_read(0xA000), 3);
        assert_eq!(m.cpu_read(0xC000), 14);
        assert_eq!(m.cpu_read(0xE000), 15);
    }

    #[test]
    fn prg_mode_1_swaps_8000_and_c000() {
        let mut m = mmc3();
        set_bank(&mut m, 0x40, 6, 9);
        set_bank(&mut m, 0x40, 7, 3);
        assert_eq!(m.cpu_read(0x8000), 14);
        assert_eq!(m.cpu_read(0xA000), 3);
        assert_eq!(m.cpu_read(0xC000), 9);
        assert_eq!(m.cpu_read(0xE000), 15);
    }

    #[test]
    fn prg_bank_wraps_to_rom_size() {
        let mut m = mmc3();
        set_bank(&mut m, 0x00, 6, 17);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn chr_mode_0_layout() {
        let mut m = mmc3();
        set_bank(&mut m, 0x00, 0, 11); // 2 KB, low bit ignored -> 10,11
        set_bank(&mut m, 0x00, 1, 20);
        set_bank(&mut m, 0x00, 2, 30);
        set_bank(&mut m, 0x00, 3, 31);
        set_bank(&mut m, 0x00, 4, 32);
        set_bank(&mut m, 0x00, 5, 33);
        assert_eq!(m.ppu_read(0x0000), 10);
        assert_eq!(m.ppu_read(0x0400), 11);
        assert_eq!(m.ppu_read(0x0800), 20);
        assert_eq!(m.ppu_read(0x0C00), 21);
        assert_eq!(m.ppu_read(0x1000), 30);
        assert_eq!(m.ppu_read(0x1400), 31);
        assert_eq!(m.ppu_read(0x1800), 32);
        assert_eq!(m.ppu_read(0x1C00), 33);
        assert_eq!(m.ppu_read(0x1FFF), 33);
    }

    #[test]
    fn chr_mode_1_layout() {
        let mut m = mmc3();
        set_bank(&mut m, 0x80, 0, 10);
        set_bank(&mut m, 0x80, 1, 20);
        set_bank(&mut m, 0x80, 2, 30);
        set_bank(&mut m, 0x80, 3, 31);
        set_bank(&mut m, 0x80, 4, 32);
        set_bank(&mut m, 0x80, 5, 33);
        assert_eq!(m.ppu_read(0x0000), 30);
        assert_eq!(m.ppu_read(0x0400), 31);
        assert_eq!(m.ppu_read(0x0800), 32);
        assert_eq!(m.ppu_read(0x0C00), 33);
        assert_eq!(m.ppu_read(0x1000), 10);
        assert_eq!(m.ppu_read(0x1400), 11);
        assert_eq!(m.ppu_read(0x1800), 20);
        assert_eq!(m.ppu_read(0x1C00), 21);
    }

    #[test]
    fn chr_ram_board_banks_within_8k() {
        let mut m = Mapper4::new(tagged_rom(4, 0x2000), vec![], Mirroring::Vertical);
        m.ppu_write(0x0000, 0x5A);
        assert_eq!(m.ppu_read(0x0000), 0x5A);
        // Bank 8 wraps onto bank 0 of the 8 KB RAM
        set_bank(&mut m, 0x80, 2, 8);
        assert_eq!(m.ppu_read(0x0000), 0x5A);
    }

    #[test]
    fn mirroring_register() {
        let mut m = mmc3();
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        m.cpu_write(0xA000, 1);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        m.cpu_write(0xA000, 0);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        let f = Mapper4::new(tagged_rom(4, 0x2000), vec![], Mirroring::FourScreen);
        assert_eq!(f.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn prg_ram_protect() {
        let mut m = mmc3();
        m.cpu_write(0x6000, 0x42);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        m.cpu_write(0xA001, 0xC0); // enabled, write-protected
        m.cpu_write(0x6000, 0x11);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        m.cpu_write(0xA001, 0x00); // disabled
        assert_eq!(m.cpu_read(0x6000), 0);
    }

    #[test]
    fn irq_counter_reloads_then_counts_down_to_pending() {
        let mut m = mmc3();
        m.cpu_write(0xC000, 3); // latch
        m.cpu_write(0xC001, 0); // reload
        m.cpu_write(0xE001, 0); // enable

        m.clock_scanline(); // reload -> 3
        assert!(!m.irq_pending());
        m.clock_scanline(); // 2
        m.clock_scanline(); // 1
        assert!(!m.irq_pending());
        m.clock_scanline(); // 0 -> pending
        assert!(Mapper::irq_pending(&m));

        m.clear_irq();
        assert!(!m.irq_pending());
        m.clock_scanline(); // counter 0 -> reload to 3
        assert_eq!(m.irq_counter, 3);
    }

    #[test]
    fn irq_disable_acknowledges_and_stops_counter_from_asserting() {
        let mut m = mmc3();
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);
        m.clock_scanline(); // latch 0 -> pending immediately
        assert!(Mapper::irq_pending(&m));
        m.cpu_write(0xE000, 0);
        assert!(!Mapper::irq_pending(&m));
        m.clock_scanline();
        assert!(!Mapper::irq_pending(&m));
    }
}
