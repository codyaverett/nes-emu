//! The `Mapper` trait: the cartridge-side view of the CPU and PPU buses.
//!
//! `System` owns a boxed mapper (inside `Cartridge`) and routes every CPU
//! access in $4020-$FFFF to `cpu_read`/`cpu_write`. The PPU is handed
//! `&mut dyn Mapper` on every `step`/register access and routes every
//! pattern-table access ($0000-$1FFF) through `ppu_read`/`ppu_write`, so CHR
//! bank switching is visible to rendering immediately. Nametable RAM stays in
//! the PPU; the PPU asks `mirroring()` per access.

use super::Mirroring;
use crate::state::{Reader, StateError, Writer};

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

    /// Side-effect-free pattern-table read used by debuggers and viewers.
    /// Must return the same bytes as `ppu_read` through the current CHR
    /// banking without clocking anything. The default is open bus.
    fn ppu_peek(&self, _addr: u16) -> u8 {
        0
    }

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

    /// Write the board's mutable state (registers, bank selects, PRG RAM,
    /// CHR RAM, IRQ counters) for a save state; it becomes the payload of
    /// the "MAPR" section (docs/debugging/SAVE_STATES.md). The default
    /// writes nothing, which pairs with the default `load_state`.
    fn save_state(&self, _w: &mut Writer) {}

    /// Restore what `save_state` wrote. The default restores nothing and
    /// logs a warning, so a board without an implementation still loads
    /// the rest of the machine (with a stale mapper).
    fn load_state(&mut self, _r: &mut Reader) -> Result<(), StateError> {
        log::warn!("This mapper has no save-state support; mapper state not restored");
        Ok(())
    }
}

/// `Mirroring` as one byte in a save state.
pub(crate) fn mirroring_to_u8(m: Mirroring) -> u8 {
    match m {
        Mirroring::Horizontal => 0,
        Mirroring::Vertical => 1,
        Mirroring::FourScreen => 2,
        Mirroring::SingleScreenLower => 3,
        Mirroring::SingleScreenUpper => 4,
    }
}

pub(crate) fn mirroring_from_u8(v: u8) -> Result<Mirroring, StateError> {
    Ok(match v {
        0 => Mirroring::Horizontal,
        1 => Mirroring::Vertical,
        2 => Mirroring::FourScreen,
        3 => Mirroring::SingleScreenLower,
        4 => Mirroring::SingleScreenUpper,
        other => {
            return Err(StateError::BadValue(
                "MAPR".into(),
                format!("mirroring code {other}"),
            ))
        }
    })
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

    /// Save-state image: a RAM flag, then the RAM contents as a blob when
    /// the board has CHR RAM. CHR ROM is never written; it comes from the
    /// cartridge.
    pub fn save_state(&self, w: &mut Writer) {
        w.bool(self.is_ram);
        if self.is_ram {
            w.blob(&self.data);
        }
    }

    pub fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        let is_ram = r.bool()?;
        if is_ram != self.is_ram {
            return Err(StateError::BadValue(
                "MAPR".into(),
                "CHR RAM flag does not match the loaded cartridge".into(),
            ));
        }
        if is_ram {
            let data = r.blob()?;
            if data.len() != self.data.len() {
                return Err(StateError::BadValue(
                    "MAPR".into(),
                    format!(
                        "CHR RAM is {} bytes, state has {}",
                        self.data.len(),
                        data.len()
                    ),
                ));
            }
            self.data.copy_from_slice(data);
        }
        Ok(())
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
