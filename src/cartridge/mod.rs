//! iNES cartridge loading and mapper construction.
//!
//! `Cartridge` parses the header, splits PRG/CHR, and builds a boxed
//! `Mapper`. All bus traffic to the cartridge goes through that trait; see
//! `mapper.rs` and docs/debugging/MAPPER_TRAIT_REFACTOR.md.

pub mod mapper;
pub mod mapper0;
pub mod mapper1;
pub mod mapper2;
pub mod mapper227;
pub mod mapper3;
pub mod mapper4;
pub mod mapper5;
pub mod mapper65;

use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result};
use std::path::Path;

pub use mapper::{Mapper, NullMapper};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
    SingleScreenLower,
    SingleScreenUpper,
}

pub struct Cartridge {
    /// iNES mapper number from the header.
    pub mapper_id: u8,
    /// Mirroring declared by the header. The live value is `mapper.mirroring()`.
    pub header_mirroring: Mirroring,
    pub battery_backed: bool,
    pub mapper: Box<dyn Mapper>,
}

impl Cartridge {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut rom_data = Vec::new();
        file.read_to_end(&mut rom_data)?;

        Self::load_from_bytes(&rom_data)
    }

    pub fn load_from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            return Err(Error::new(ErrorKind::InvalidData, "ROM file too small"));
        }

        if &data[0..4] != b"NES\x1A" {
            return Err(Error::new(ErrorKind::InvalidData, "Invalid NES header"));
        }

        let prg_rom_size = data[4] as usize * 0x4000;
        let chr_rom_size = data[5] as usize * 0x2000;

        let flags_6 = data[6];
        let flags_7 = data[7];

        let mirroring = if (flags_6 & 0x08) != 0 {
            Mirroring::FourScreen
        } else if (flags_6 & 0x01) != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };

        let battery_backed = (flags_6 & 0x02) != 0;
        let trainer_present = (flags_6 & 0x04) != 0;

        // Archaic iNES headers (pre-0.7 tools, and dumps that carry a
        // "DiskDude" style signature in bytes 7-15) only define byte 6.
        // Byte 7 then holds ASCII, and its upper nibble would corrupt the
        // mapper number: roms/Tetris.nes reads 0x44 there and became mapper
        // 65 instead of 1. The standard heuristic (nesdev "iNES" page): if
        // bytes 12-15 are nonzero, or byte 7 bits 2-3 read 01, trust only
        // the low mapper nibble from byte 6. NES 2.0 (byte 7 bits 2-3 = 10)
        // is checked first: bytes 12-15 are real fields there and byte 7's
        // mapper nibble is valid, so it must not trip the heuristic.
        let nes2 = (flags_7 & 0x0C) == 0x08;
        let archaic = !nes2 && (data[12..16].iter().any(|&b| b != 0) || (flags_7 & 0x0C) == 0x04);
        let mapper_id = if archaic {
            let full = (flags_7 & 0xF0) | ((flags_6 & 0xF0) >> 4);
            let low = (flags_6 & 0xF0) >> 4;
            if full != low {
                log::warn!(
                    "Archaic iNES header (bytes 7-15 = {:02x?}): ignoring byte 7, mapper {} not {}",
                    &data[7..16],
                    low,
                    full
                );
            }
            low
        } else {
            (flags_7 & 0xF0) | ((flags_6 & 0xF0) >> 4)
        };

        let header_size = 16;
        let trainer_size = if trainer_present { 512 } else { 0 };
        let prg_rom_start = header_size + trainer_size;
        let chr_rom_start = prg_rom_start + prg_rom_size;

        if data.len() < chr_rom_start + chr_rom_size {
            return Err(Error::new(ErrorKind::InvalidData, "ROM file truncated"));
        }

        let prg_rom = data[prg_rom_start..prg_rom_start + prg_rom_size].to_vec();
        // Zero CHR banks means the board carries CHR RAM; the mapper allocates it.
        let chr_rom = data[chr_rom_start..chr_rom_start + chr_rom_size].to_vec();

        let mapper = Self::build_mapper(mapper_id, prg_rom, chr_rom, mirroring);

        Ok(Cartridge {
            mapper_id,
            header_mirroring: mirroring,
            battery_backed,
            mapper,
        })
    }

    /// Construct the mapper for `mapper_id`. Unknown numbers fall back to
    /// NROM behaviour with a warning, as before the trait existed.
    pub fn build_mapper(
        mapper_id: u8,
        prg_rom: Vec<u8>,
        chr_rom: Vec<u8>,
        mirroring: Mirroring,
    ) -> Box<dyn Mapper> {
        match mapper_id {
            0 => Box::new(mapper0::Mapper0::new(prg_rom, chr_rom, mirroring)),
            1 => Box::new(mapper1::Mapper1::new(prg_rom, chr_rom, mirroring)),
            2 => Box::new(mapper2::Mapper2::new(prg_rom, chr_rom, mirroring)),
            3 => Box::new(mapper3::Mapper3::new(prg_rom, chr_rom, mirroring)),
            4 => Box::new(mapper4::Mapper4::new(prg_rom, chr_rom, mirroring)),
            5 => Box::new(mapper5::Mapper5::new(prg_rom, chr_rom, mirroring)),
            65 => Box::new(mapper65::Mapper65::new(prg_rom, chr_rom, mirroring)),
            227 => Box::new(mapper227::Mapper227::new(prg_rom, chr_rom, mirroring)),
            other => {
                log::warn!(
                    "Unsupported mapper {}: falling back to NROM behaviour",
                    other
                );
                Box::new(mapper0::Mapper0::new(prg_rom, chr_rom, mirroring))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ines(mapper: u8, prg_banks: u8, chr_banks: u8, flags6_low: u8) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = prg_banks;
        data[5] = chr_banks;
        data[6] = ((mapper & 0x0F) << 4) | flags6_low;
        data[7] = mapper & 0xF0;
        for bank in 0..prg_banks {
            data.extend(vec![bank; 0x4000]);
        }
        for bank in 0..chr_banks {
            data.extend(vec![0x80 | bank; 0x2000]);
        }
        data
    }

    #[test]
    fn parses_header_and_builds_mapper() {
        let cart = Cartridge::load_from_bytes(&ines(3, 2, 2, 0x03)).unwrap();
        assert_eq!(cart.mapper_id, 3);
        assert!(cart.battery_backed);
        assert_eq!(cart.header_mirroring, Mirroring::Vertical);
        let mut mapper = cart.mapper;
        assert_eq!(mapper.mirroring(), Mirroring::Vertical);
        assert_eq!(mapper.cpu_read(0xC000), 1);
        assert_eq!(mapper.ppu_read(0x0000), 0x80);
        mapper.cpu_write(0x8000, 1);
        assert_eq!(mapper.ppu_read(0x0000), 0x81);
    }

    #[test]
    fn zero_chr_banks_allocates_writable_chr_ram() {
        let cart = Cartridge::load_from_bytes(&ines(0, 1, 0, 0x00)).unwrap();
        let mut mapper = cart.mapper;
        assert_eq!(mapper.ppu_read(0x0100), 0);
        mapper.ppu_write(0x0100, 0x3C);
        assert_eq!(mapper.ppu_read(0x0100), 0x3C);
    }

    #[test]
    fn unknown_mapper_falls_back_to_nrom() {
        let cart = Cartridge::load_from_bytes(&ines(200, 2, 1, 0x00)).unwrap();
        assert_eq!(cart.mapper_id, 200);
        let mut mapper = cart.mapper;
        assert_eq!(mapper.cpu_read(0x8000), 0);
        assert_eq!(mapper.cpu_read(0xC000), 1);
    }

    #[test]
    fn archaic_header_signature_does_not_corrupt_mapper_number() {
        // Tetris (MMC1, vertical mirroring) as dumped with a DiskDude
        // signature in bytes 7-15: byte 7 = 0x44 used to yield mapper 65.
        let mut data = ines(1, 8, 16, 0x01);
        data[7..16].copy_from_slice(b"DiskDude!");
        let cart = Cartridge::load_from_bytes(&data).unwrap();
        assert_eq!(cart.mapper_id, 1);
        assert_eq!(cart.header_mirroring, Mirroring::Vertical);
        // MMC1 power-on: PRG mode 3, last 16 KB bank fixed at $C000, and a
        // serial CHR bank load reaches the PPU (H3001 would not respond).
        let mut mapper = cart.mapper;
        assert_eq!(mapper.cpu_read(0xC000), 7);
        for i in 0..5 {
            mapper.cpu_write(0xA000, (2u8 >> i) & 1); // 8 KB mode: banks 2,3
        }
        assert_eq!(mapper.ppu_read(0x0000), 0x81);
    }

    #[test]
    fn archaic_flag_bits_alone_ignore_byte_7() {
        // Byte 7 bits 2-3 = 01 marks an archaic header even with clean padding.
        let mut data = ines(1, 2, 1, 0x00);
        data[7] = 0x44;
        assert_eq!(Cartridge::load_from_bytes(&data).unwrap().mapper_id, 1);
        // A clean header still honours the upper nibble.
        data[7] = 0x40;
        assert_eq!(Cartridge::load_from_bytes(&data).unwrap().mapper_id, 65);
    }

    #[test]
    fn nes2_header_keeps_upper_mapper_nibble_despite_nonzero_tail() {
        // NES 2.0: byte 7 bits 2-3 = 10 and bytes 12-15 are real fields
        // (byte 15 = 1 is the standard-controller expansion device).
        let mut data = ines(65, 2, 1, 0x00);
        data[7] |= 0x08;
        data[15] = 0x01;
        assert_eq!(Cartridge::load_from_bytes(&data).unwrap().mapper_id, 65);
    }

    #[test]
    fn rejects_bad_header_and_truncation() {
        assert!(Cartridge::load_from_bytes(&[0; 8]).is_err());
        let mut bad = ines(0, 1, 1, 0);
        bad[0] = b'X';
        assert!(Cartridge::load_from_bytes(&bad).is_err());
        let mut short = ines(0, 1, 1, 0);
        short.truncate(short.len() - 1);
        assert!(Cartridge::load_from_bytes(&short).is_err());
    }
}
