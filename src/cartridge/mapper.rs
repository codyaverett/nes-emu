//! The `Mapper` trait: the cartridge-side view of the CPU and PPU buses.
//!
//! `System` owns a boxed mapper (inside `Cartridge`) and routes every CPU
//! access in $4020-$FFFF to `cpu_read`/`cpu_write`. The PPU is handed
//! `&mut dyn Mapper` on every `step`/register access and routes every
//! pattern-table access ($0000-$1FFF) through `ppu_read`/`ppu_write`, so CHR
//! bank switching is visible to rendering immediately. Nametable RAM stays in
//! the PPU; the PPU asks `mirroring()` per access.

use super::Mirroring;

pub trait Mapper: Send {
    /// CPU bus read for $4020-$FFFF (PRG ROM, PRG RAM, mapper registers).
    fn cpu_read(&mut self, addr: u16) -> u8;

    /// CPU bus write for $4020-$FFFF.
    fn cpu_write(&mut self, addr: u16, value: u8);

    /// Side-effect-free CPU read used by debuggers and the test harness.
    /// Must return the same bytes as `cpu_read` for PRG RAM and PRG ROM,
    /// but must not touch mapper registers that change state when read.
    fn cpu_peek(&self, addr: u16) -> u8;

    /// PPU bus read for $0000-$1FFF (pattern tables, CHR ROM or CHR RAM).
    fn ppu_read(&mut self, addr: u16) -> u8;

    /// PPU bus write for $0000-$1FFF. Ignored by CHR ROM boards.
    fn ppu_write(&mut self, addr: u16, value: u8);

    /// Current nametable mirroring. Queried per nametable access.
    fn mirroring(&self) -> Mirroring;

    /// True while the mapper is asserting its IRQ line.
    fn irq_pending(&self) -> bool {
        false
    }

    /// Acknowledge the mapper IRQ.
    fn clear_irq(&mut self) {}

    /// Scanline clock for mappers that count scanlines directly (MMC5).
    /// Called by the PPU once per rendered scanline. No-op for everything
    /// else.
    fn clock_scanline(&mut self) {}

    /// Rising edge of PPU address line A12, already filtered by the PPU so
    /// that A12 must have been low for several PPU cycles first. MMC3 clocks
    /// its IRQ counter here. No-op for everything else.
    fn ppu_a12_rise(&mut self) {}

    /// Borrow the board's PRG RAM ($6000-$7FFF) for battery persistence.
    /// `None` for boards without PRG RAM. Returns the raw array even when
    /// the board has gated it off the bus (MMC3 $A001), since that gate
    /// controls CPU access, not what the battery keeps.
    fn prg_ram(&self) -> Option<&[u8]> {
        None
    }

    /// Mutable counterpart of `prg_ram`, used to restore a save file.
    fn prg_ram_mut(&mut self) -> Option<&mut [u8]> {
        None
    }
}

/// Mapper used when no cartridge is loaded: open bus everywhere.
#[derive(Default)]
pub struct NullMapper;

impl Mapper for NullMapper {
    fn cpu_read(&mut self, _addr: u16) -> u8 {
        0
    }

    fn cpu_write(&mut self, _addr: u16, _value: u8) {}

    fn cpu_peek(&self, _addr: u16) -> u8 {
        0
    }

    fn ppu_read(&mut self, _addr: u16) -> u8 {
        0
    }

    fn ppu_write(&mut self, _addr: u16, _value: u8) {}

    fn mirroring(&self) -> Mirroring {
        Mirroring::Horizontal
    }
}

/// CHR storage shared by the simple mappers: either CHR ROM from the image or
/// an 8 KB CHR RAM allocated when the header reports zero CHR banks.
pub(crate) struct Chr {
    pub data: Vec<u8>,
    pub is_ram: bool,
}

impl Chr {
    pub fn new(chr_rom: Vec<u8>) -> Self {
        if chr_rom.is_empty() {
            Chr {
                data: vec![0; 0x2000],
                is_ram: true,
            }
        } else {
            Chr {
                data: chr_rom,
                is_ram: false,
            }
        }
    }

    /// Read from an absolute CHR offset, wrapping to the CHR size.
    pub fn read(&self, offset: usize) -> u8 {
        self.data[offset % self.data.len()]
    }

    pub fn write(&mut self, offset: usize, value: u8) {
        if self.is_ram {
            let len = self.data.len();
            self.data[offset % len] = value;
        }
    }
}

/// Read from PRG ROM at an absolute offset, wrapping to the ROM size.
pub(crate) fn prg_read(prg_rom: &[u8], offset: usize) -> u8 {
    if prg_rom.is_empty() {
        0
    } else {
        prg_rom[offset % prg_rom.len()]
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    /// Build a ROM image of `banks` banks of `bank_size` bytes where every
    /// byte of bank `i` equals `i`, so a read tells you which bank it hit.
    pub fn tagged_rom(banks: usize, bank_size: usize) -> Vec<u8> {
        let mut rom = Vec::with_capacity(banks * bank_size);
        for bank in 0..banks {
            rom.extend(vec![bank as u8; bank_size]);
        }
        rom
    }
}
