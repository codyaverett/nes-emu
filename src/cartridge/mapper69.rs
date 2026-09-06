//! Mapper 69 (Sunsoft FME-7 / 5A / 5B).
//!
//! Used by Batman: Return of the Joker, Gimmick!, Hebereke, Gremlins 2.
//!
//! Two registers: $8000-$9FFF selects a command (bits 0-3) and $A000-$BFFF
//! writes its parameter. $C000-$FFFF is the 5B audio chip, not modelled.
//!
//!   $0-$7  1 KB CHR bank for PPU $0000 + n * $400
//!   $8     $6000-$7FFF: "ERbB BBBB". R = 1 selects PRG RAM (E = 1 enables
//!          it, otherwise the range is open bus), R = 0 maps 8 KB PRG ROM
//!          bank bBBBBB
//!   $9-$B  8 KB PRG ROM bank (bits 0-5) at $8000, $A000, $C000
//!          $E000-$FFFF is fixed to the last 8 KB bank
//!   $C     mirroring (bits 0-1): 0 horizontal, 1 vertical,
//!          2 single-screen lower, 3 single-screen upper
//!   $D     IRQ control: bit 0 enables the IRQ output, bit 7 enables the
//!          counter. Every write acknowledges a pending IRQ
//!   $E/$F  IRQ counter low / high byte
//!
//! The 16-bit counter decrements once per CPU cycle while enabled
//! (`cpu_clock`, called from `System::tick`) and raises the IRQ on the
//! $0000 to $FFFF wrap when the IRQ output is enabled. It keeps counting
//! after the wrap; the IRQ line stays high until a command $D write.

use super::mapper::{prg_read, Chr, Mapper};
use super::Mirroring;

pub struct Mapper69 {
    prg_rom: Vec<u8>,
    chr: Chr,
    prg_ram: [u8; 0x2000],

    command: u8,
    chr_banks: [u8; 8],
    /// Raw command $8 parameter: RAM enable, RAM/ROM select, bank.
    prg_6000: u8,
    /// 8 KB PRG banks for $8000, $A000, $C000.
    prg_banks: [u8; 3],
    mirroring: u8,

    irq_enabled: bool,
    irq_counter_enabled: bool,
    irq_counter: u16,
    irq_pending: bool,
}

impl Mapper69 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        Mapper69 {
            prg_rom,
            chr: Chr::new(chr_rom),
            prg_ram: [0; 0x2000],
            command: 0,
            chr_banks: [0; 8],
            prg_6000: 0,
            prg_banks: [0; 3],
            mirroring: match mirroring {
                Mirroring::Vertical => 1,
                Mirroring::SingleScreenLower => 2,
                Mirroring::SingleScreenUpper => 3,
                _ => 0,
            },
            irq_enabled: false,
            irq_counter_enabled: false,
            irq_counter: 0,
            irq_pending: false,
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let slot = ((addr & 0x1FFF) / 0x400) as usize;
        self.chr_banks[slot] as usize * 0x400 + (addr & 0x3FF) as usize
    }

    fn ram_selected(&self) -> bool {
        self.prg_6000 & 0x40 != 0
    }

    fn ram_enabled(&self) -> bool {
        self.prg_6000 & 0x80 != 0
    }

    fn write_parameter(&mut self, value: u8) {
        match self.command {
            0x0..=0x7 => self.chr_banks[self.command as usize] = value,
            0x8 => self.prg_6000 = value,
            0x9..=0xB => self.prg_banks[(self.command - 0x9) as usize] = value & 0x3F,
            0xC => self.mirroring = value & 0x03,
            0xD => {
                self.irq_enabled = value & 0x01 != 0;
                self.irq_counter_enabled = value & 0x80 != 0;
                self.irq_pending = false;
            }
            0xE => self.irq_counter = (self.irq_counter & 0xFF00) | value as u16,
            0xF => self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8),
            _ => unreachable!("command is masked to four bits"),
        }
    }
}

impl Mapper for Mapper69 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        self.cpu_peek(addr)
    }

    fn cpu_peek(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => {
                if self.ram_selected() {
                    if self.ram_enabled() {
                        self.prg_ram[(addr - 0x6000) as usize]
                    } else {
                        0
                    }
                } else {
                    let bank = (self.prg_6000 & 0x3F) as usize;
                    prg_read(&self.prg_rom, bank * 0x2000 + (addr - 0x6000) as usize)
                }
            }
            0x8000..=0xDFFF => {
                let slot = ((addr - 0x8000) / 0x2000) as usize;
                let bank = self.prg_banks[slot] as usize;
                prg_read(&self.prg_rom, bank * 0x2000 + (addr & 0x1FFF) as usize)
            }
            0xE000..=0xFFFF => {
                let base = self.prg_rom.len().saturating_sub(0x2000);
                prg_read(&self.prg_rom, base + (addr & 0x1FFF) as usize)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.ram_selected() && self.ram_enabled() {
                    self.prg_ram[(addr - 0x6000) as usize] = value;
                }
            }
            0x8000..=0x9FFF => self.command = value & 0x0F,
            0xA000..=0xBFFF => self.write_parameter(value),
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
        match self.mirroring {
            0 => Mirroring::Horizontal,
            1 => Mirroring::Vertical,
            2 => Mirroring::SingleScreenLower,
            _ => Mirroring::SingleScreenUpper,
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq_pending
    }

    fn clear_irq(&mut self) {
        self.irq_pending = false;
    }

    fn cpu_clock(&mut self) {
        if !self.irq_counter_enabled {
            return;
        }
        let wraps = self.irq_counter == 0;
        self.irq_counter = self.irq_counter.wrapping_sub(1);
        if wraps && self.irq_enabled {
            self.irq_pending = true;
        }
    }

    fn prg_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.prg_ram)
    }

    // State: command u8, chr_banks 8 x u8, prg_6000 u8, prg_banks 3 x u8,
    // mirroring u8, irq_enabled bool, irq_counter_enabled bool,
    // irq_counter u16, irq_pending bool, PRG RAM 8 KB, CHR.
    fn save_state(&self, w: &mut crate::state::Writer) {
        w.u8(self.command);
        w.bytes(&self.chr_banks);
        w.u8(self.prg_6000);
        w.bytes(&self.prg_banks);
        w.u8(self.mirroring);
        w.bool(self.irq_enabled);
        w.bool(self.irq_counter_enabled);
        w.u16(self.irq_counter);
        w.bool(self.irq_pending);
        w.bytes(&self.prg_ram);
        self.chr.save_state(w);
    }

    fn load_state(&mut self, r: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        self.command = r.u8()? & 0x0F;
        r.bytes(&mut self.chr_banks)?;
        self.prg_6000 = r.u8()?;
        r.bytes(&mut self.prg_banks)?;
        self.mirroring = r.u8()? & 0x03;
        self.irq_enabled = r.bool()?;
        self.irq_counter_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        self.irq_pending = r.bool()?;
        r.bytes(&mut self.prg_ram)?;
        self.chr.load_state(r)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapper::test_util::tagged_rom;
    use super::*;

    /// 32 x 8 KB PRG, 64 x 1 KB CHR (tagged 0x80 | bank).
    fn fme7() -> Mapper69 {
        let chr: Vec<u8> = tagged_rom(64, 0x400).iter().map(|b| b | 0x80).collect();
        Mapper69::new(tagged_rom(32, 0x2000), chr, Mirroring::Horizontal)
    }

    fn cmd(m: &mut Mapper69, command: u8, value: u8) {
        m.cpu_write(0x8000, command);
        m.cpu_write(0xA000, value);
    }

    #[test]
    fn power_on_layout() {
        let mut m = fme7();
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xA000), 0);
        assert_eq!(m.cpu_read(0xC000), 0);
        assert_eq!(m.cpu_read(0xE000), 31);
        assert_eq!(m.cpu_read(0xFFFF), 31);
        // Command 8 = 0: PRG ROM bank 0 at $6000.
        assert_eq!(m.cpu_read(0x6000), 0);
        assert_eq!(m.ppu_read(0x0000), 0x80);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        assert!(!m.irq_pending());
    }

    #[test]
    fn chr_1k_banks() {
        let mut m = fme7();
        for slot in 0..8u8 {
            cmd(&mut m, slot, 40 + slot);
        }
        for slot in 0..8u16 {
            let addr = slot * 0x400;
            assert_eq!(m.ppu_read(addr), 0x80 | (40 + slot as u8));
            assert_eq!(m.ppu_read(addr + 0x3FF), 0x80 | (40 + slot as u8));
            assert_eq!(m.ppu_peek(addr), m.ppu_read(addr));
        }
        // Wraps to the image size.
        cmd(&mut m, 0, 65);
        assert_eq!(m.ppu_read(0x0000), 0x81);
    }

    #[test]
    fn prg_8k_banks_with_last_fixed() {
        let mut m = fme7();
        cmd(&mut m, 0x9, 3);
        cmd(&mut m, 0xA, 7);
        cmd(&mut m, 0xB, 12);
        assert_eq!(m.cpu_read(0x8000), 3);
        assert_eq!(m.cpu_read(0x9FFF), 3);
        assert_eq!(m.cpu_read(0xA000), 7);
        assert_eq!(m.cpu_read(0xC000), 12);
        assert_eq!(m.cpu_read(0xDFFF), 12);
        assert_eq!(m.cpu_read(0xE000), 31);
        // Bits 6-7 are ignored; out-of-range banks wrap.
        cmd(&mut m, 0x9, 0xC5);
        assert_eq!(m.cpu_read(0x8000), 5);
        cmd(&mut m, 0x9, 33);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn command_register_uses_low_nibble_only() {
        let mut m = fme7();
        m.cpu_write(0x9FFF, 0xF9);
        m.cpu_write(0xBFFF, 4);
        assert_eq!(m.cpu_read(0x8000), 4);
        // Audio registers are not mapper registers.
        m.cpu_write(0xC000, 0xA);
        m.cpu_write(0xE000, 9);
        assert_eq!(m.cpu_read(0xA000), 0);
    }

    #[test]
    fn slot_6000_rom_ram_and_open_bus() {
        let mut m = fme7();
        // ROM bank at $6000.
        cmd(&mut m, 0x8, 0x05);
        assert_eq!(m.cpu_read(0x6000), 5);
        assert_eq!(m.cpu_read(0x7FFF), 5);
        m.cpu_write(0x6000, 0x11); // ROM: no effect on RAM
                                   // RAM selected and enabled.
        cmd(&mut m, 0x8, 0xC0);
        assert_eq!(m.cpu_read(0x6000), 0);
        m.cpu_write(0x6000, 0x42);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        assert_eq!(m.prg_ram().unwrap()[0], 0x42);
        // RAM selected but disabled: open bus, writes dropped.
        cmd(&mut m, 0x8, 0x40);
        assert_eq!(m.cpu_read(0x6000), 0);
        m.cpu_write(0x6000, 0x99);
        cmd(&mut m, 0x8, 0xC0);
        assert_eq!(m.cpu_read(0x6000), 0x42);
        // Bank bits are ignored while RAM is selected.
        cmd(&mut m, 0x8, 0xC7);
        assert_eq!(m.cpu_peek(0x6000), 0x42);
    }

    #[test]
    fn mirroring_command() {
        let mut m = fme7();
        cmd(&mut m, 0xC, 0);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        cmd(&mut m, 0xC, 1);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        cmd(&mut m, 0xC, 2);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenLower);
        cmd(&mut m, 0xC, 0xFF);
        assert_eq!(m.mirroring(), Mirroring::SingleScreenUpper);
        let v = Mapper69::new(tagged_rom(4, 0x2000), vec![], Mirroring::Vertical);
        assert_eq!(v.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn irq_fires_on_wrap_after_exact_cycle_count() {
        let mut m = fme7();
        cmd(&mut m, 0xE, 0x02);
        cmd(&mut m, 0xF, 0x00);
        cmd(&mut m, 0xD, 0x81);
        // 2 -> 1 -> 0 -> FFFF: the third clock fires.
        m.cpu_clock();
        m.cpu_clock();
        assert!(!m.irq_pending());
        m.cpu_clock();
        assert!(m.irq_pending());
        assert_eq!(m.irq_counter, 0xFFFF);
        // Keeps counting; the line stays asserted until acknowledged.
        m.cpu_clock();
        assert_eq!(m.irq_counter, 0xFFFE);
        assert!(m.irq_pending());
    }

    #[test]
    fn irq_counter_takes_high_byte() {
        let mut m = fme7();
        cmd(&mut m, 0xE, 0x00);
        cmd(&mut m, 0xF, 0x01);
        cmd(&mut m, 0xD, 0x81);
        for _ in 0..0x100 {
            m.cpu_clock();
        }
        assert!(!m.irq_pending());
        m.cpu_clock();
        assert!(m.irq_pending());
    }

    #[test]
    fn counter_disabled_does_not_count_and_irq_disabled_does_not_assert() {
        let mut m = fme7();
        cmd(&mut m, 0xE, 0);
        cmd(&mut m, 0xF, 0);
        // IRQ output on, counter off: nothing moves.
        cmd(&mut m, 0xD, 0x01);
        for _ in 0..10 {
            m.cpu_clock();
        }
        assert_eq!(m.irq_counter, 0);
        assert!(!m.irq_pending());
        // Counter on, IRQ output off: wraps silently.
        cmd(&mut m, 0xD, 0x80);
        m.cpu_clock();
        assert_eq!(m.irq_counter, 0xFFFF);
        assert!(!m.irq_pending());
    }

    #[test]
    fn command_d_write_acknowledges_and_clear_irq_works() {
        let mut m = fme7();
        cmd(&mut m, 0xE, 0);
        cmd(&mut m, 0xF, 0);
        cmd(&mut m, 0xD, 0x81);
        m.cpu_clock();
        assert!(Mapper::irq_pending(&m));
        cmd(&mut m, 0xD, 0x81);
        assert!(!Mapper::irq_pending(&m));
        m.cpu_clock();
        assert!(!Mapper::irq_pending(&m), "no wrap this cycle");
        cmd(&mut m, 0xE, 0);
        cmd(&mut m, 0xF, 0);
        m.cpu_clock();
        assert!(Mapper::irq_pending(&m));
        m.clear_irq();
        assert!(!Mapper::irq_pending(&m));
    }

    #[test]
    fn save_state_round_trips_banks_and_irq_counter() {
        use crate::state::{Reader, Writer};
        let mut m = fme7();
        for slot in 0..8u8 {
            cmd(&mut m, slot, 10 + slot);
        }
        cmd(&mut m, 0x8, 0xC0);
        m.cpu_write(0x6010, 0x5A);
        cmd(&mut m, 0x9, 3);
        cmd(&mut m, 0xA, 7);
        cmd(&mut m, 0xB, 12);
        cmd(&mut m, 0xC, 1);
        cmd(&mut m, 0xE, 0x05);
        cmd(&mut m, 0xF, 0x00);
        cmd(&mut m, 0xD, 0x81);
        m.cpu_clock();
        m.cpu_clock(); // counter = 3
        let mut w = Writer::new();
        m.save_state(&mut w);
        let bytes = w.into_bytes();
        let snapshot = |m: &mut Mapper69| {
            let mut v = Vec::new();
            for addr in (0x6000..=0xFFFF).step_by(0x2000) {
                v.push(m.cpu_read(addr));
            }
            for addr in (0x0000..0x2000).step_by(0x400) {
                v.push(m.ppu_read(addr));
            }
            v
        };
        let before = snapshot(&mut m);

        for slot in 0..8u8 {
            cmd(&mut m, slot, 20 + slot);
        }
        cmd(&mut m, 0x8, 0x02);
        cmd(&mut m, 0x9, 1);
        cmd(&mut m, 0xC, 3);
        for _ in 0..4 {
            m.cpu_clock();
        }
        assert!(m.irq_pending());
        assert_ne!(snapshot(&mut m), before);

        let mut r = Reader::new(&bytes);
        m.load_state(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(snapshot(&mut m), before);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        assert_eq!(m.cpu_read(0x6010), 0x5A);
        assert!(!m.irq_pending());
        // Counter was 3: three clocks reach 0, the fourth wraps.
        for _ in 0..3 {
            m.cpu_clock();
        }
        assert!(!m.irq_pending());
        m.cpu_clock();
        assert!(m.irq_pending());
        let mut again = Writer::new();
        m.save_state(&mut again);
        assert_ne!(again.into_bytes(), bytes, "the IRQ state moved on");
    }
}
