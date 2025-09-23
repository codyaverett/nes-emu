use std::fs::File;
use std::io::{Read, Result, Error, ErrorKind};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
    _SingleScreenLower,
    _SingleScreenUpper,
}

pub struct Cartridge {
    pub prg_rom: Vec<u8>,
    pub chr_rom: Vec<u8>,
    pub mapper: u8,
    pub _mirroring: Mirroring,
    pub _battery_backed: bool,
    pub prg_ram: Vec<u8>,
    
    // MMC1 state (Mapper 1)
    mmc1_shift_register: u8,
    mmc1_shift_count: u8,
    mmc1_control: u8,
    mmc1_chr_bank_0: u8,
    mmc1_chr_bank_1: u8,
    mmc1_prg_bank: u8,
    
    // Mapper 65 state (Irem H3001)
    m65_prg_banks: [u8; 3],
    m65_chr_banks: [u8; 8],
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
        
        let mapper = (flags_7 & 0xF0) | ((flags_6 & 0xF0) >> 4);
        
        let prg_ram_size = if data[8] == 0 { 0x2000 } else { data[8] as usize * 0x2000 };
        
        let header_size = 16;
        let trainer_size = if trainer_present { 512 } else { 0 };
        let prg_rom_start = header_size + trainer_size;
        let chr_rom_start = prg_rom_start + prg_rom_size;
        
        if data.len() < chr_rom_start + chr_rom_size {
            return Err(Error::new(ErrorKind::InvalidData, "ROM file truncated"));
        }
        
        let prg_rom = data[prg_rom_start..prg_rom_start + prg_rom_size].to_vec();
        let chr_rom = if chr_rom_size > 0 {
            data[chr_rom_start..chr_rom_start + chr_rom_size].to_vec()
        } else {
            vec![0; 0x2000]
        };
        
        Ok(Cartridge {
            prg_rom,
            chr_rom,
            mapper,
            _mirroring: mirroring,
            _battery_backed: battery_backed,
            prg_ram: vec![0; prg_ram_size],
            
            // Initialize MMC1 state
            mmc1_shift_register: 0,
            mmc1_shift_count: 0,
            mmc1_control: 0x0C, // Default: 16KB PRG mode, fixed high bank
            mmc1_chr_bank_0: 0,
            mmc1_chr_bank_1: 0,
            mmc1_prg_bank: 0,
            
            // Initialize Mapper 65 state
            m65_prg_banks: [0, 1, 2], // Default banks
            m65_chr_banks: [0, 1, 2, 3, 4, 5, 6, 7],
        })
    }

    pub fn read_prg(&self, addr: u16) -> u8 {
        match self.mapper {
            0 => {
                // Mapper 0: 16KB or 32KB PRG ROM
                if self.prg_rom.len() == 0x4000 {
                    // 16KB: Mirror at 0x8000-0xBFFF and 0xC000-0xFFFF
                    self.prg_rom[(addr & 0x3FFF) as usize]
                } else {
                    // 32KB: Direct mapping
                    if (addr as usize) < self.prg_rom.len() {
                        self.prg_rom[addr as usize]
                    } else {
                        0xFF
                    }
                }
            }
            1 => {
                // MMC1 Mapper
                let prg_mode = (self.mmc1_control >> 2) & 0x03;
                let prg_banks = self.prg_rom.len() / 0x4000;
                
                match prg_mode {
                    0 | 1 => {
                        // 32KB mode: ignore low bit of bank number
                        let bank = (self.mmc1_prg_bank & 0xFE) as usize;
                        let offset = bank * 0x4000 + (addr as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    2 => {
                        // Fix first bank at $8000, switch 16KB bank at $C000
                        if addr < 0x4000 {
                            // First bank fixed
                            self.prg_rom[addr as usize]
                        } else {
                            // Switchable bank
                            let bank = self.mmc1_prg_bank as usize;
                            let offset = bank * 0x4000 + ((addr - 0x4000) as usize);
                            if offset < self.prg_rom.len() {
                                self.prg_rom[offset]
                            } else {
                                0
                            }
                        }
                    }
                    3 => {
                        // Switch 16KB bank at $8000, fix last bank at $C000
                        if addr < 0x4000 {
                            // Switchable bank
                            let bank = self.mmc1_prg_bank as usize;
                            let offset = bank * 0x4000 + (addr as usize);
                            if offset < self.prg_rom.len() {
                                self.prg_rom[offset]
                            } else {
                                0
                            }
                        } else {
                            // Last bank fixed
                            let last_bank = prg_banks - 1;
                            let offset = last_bank * 0x4000 + ((addr - 0x4000) as usize);
                            if offset < self.prg_rom.len() {
                                self.prg_rom[offset]
                            } else {
                                0
                            }
                        }
                    }
                    _ => 0
                }
            }
            65 => {
                // Mapper 65 (Irem H3001)
                match addr {
                    0x0000..=0x1FFF => {
                        // Bank 0: switchable
                        let bank = self.m65_prg_banks[0] as usize;
                        let offset = bank * 0x2000 + (addr as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x2000..=0x3FFF => {
                        // Bank 1: switchable
                        let bank = self.m65_prg_banks[1] as usize;
                        let offset = bank * 0x2000 + ((addr - 0x2000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x4000..=0x5FFF => {
                        // Bank 2: switchable
                        let bank = self.m65_prg_banks[2] as usize;
                        let offset = bank * 0x2000 + ((addr - 0x4000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x6000..=0x7FFF => {
                        // Last bank: fixed to last 8KB
                        let last_bank = (self.prg_rom.len() / 0x2000) - 1;
                        let offset = last_bank * 0x2000 + ((addr - 0x6000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    _ => 0
                }
            }
            _ => {
                // Basic fallback for unsupported mappers
                // Treat as 32KB ROM with simple mirroring for smaller ROMs
                if self.prg_rom.is_empty() {
                    return 0;
                }
                
                let rom_size = self.prg_rom.len();
                if rom_size <= 0x4000 {
                    // 16KB or smaller: mirror across the entire range
                    self.prg_rom[(addr as usize) % rom_size]
                } else if rom_size <= 0x8000 {
                    // 32KB: direct mapping with bounds checking
                    if (addr as usize) < rom_size {
                        self.prg_rom[addr as usize]
                    } else {
                        // Mirror the last bank
                        self.prg_rom[((addr as usize) % 0x4000) + rom_size - 0x4000]
                    }
                } else {
                    // Larger ROMs: map the last 32KB to 0x8000-0xFFFF range
                    // This gives the best chance of hitting the reset vector
                    let bank_offset = rom_size - 0x8000;
                    let mapped_addr = (addr as usize) + bank_offset;
                    if mapped_addr < rom_size {
                        self.prg_rom[mapped_addr]
                    } else {
                        0
                    }
                }
            }
        }
    }

    pub fn write_prg(&mut self, addr: u16, value: u8) {
        match self.mapper {
            0 => {
                log::warn!("Attempting to write to ROM at {:04X}", addr);
            }
            1 => {
                // MMC1 Register writes
                if value & 0x80 != 0 {
                    // Reset sequence
                    self.mmc1_shift_register = 0;
                    self.mmc1_shift_count = 0;
                    self.mmc1_control |= 0x0C; // Set to mode 3
                } else {
                    // Normal write
                    self.mmc1_shift_register = (self.mmc1_shift_register >> 1) | ((value & 1) << 4);
                    self.mmc1_shift_count += 1;
                    
                    if self.mmc1_shift_count == 5 {
                        // Complete write
                        match addr & 0x6000 {
                            0x0000 => {
                                // Control register ($8000-$9FFF)
                                self.mmc1_control = self.mmc1_shift_register;
                            }
                            0x2000 => {
                                // CHR bank 0 ($A000-$BFFF)
                                self.mmc1_chr_bank_0 = self.mmc1_shift_register;
                            }
                            0x4000 => {
                                // CHR bank 1 ($C000-$DFFF)
                                self.mmc1_chr_bank_1 = self.mmc1_shift_register;
                            }
                            0x6000 => {
                                // PRG bank ($E000-$FFFF)
                                self.mmc1_prg_bank = self.mmc1_shift_register & 0x0F;
                            }
                            _ => {}
                        }
                        self.mmc1_shift_register = 0;
                        self.mmc1_shift_count = 0;
                    }
                }
            }
            65 => {
                // Mapper 65 Register writes
                match addr {
                    0x0000 => self.m65_prg_banks[0] = value,
                    0x2000 => self.m65_prg_banks[1] = value,
                    0x4000 => self.m65_prg_banks[2] = value,
                    0x1000 => self.m65_chr_banks[0] = value,
                    0x1001 => self.m65_chr_banks[1] = value,
                    0x1002 => self.m65_chr_banks[2] = value,
                    0x1003 => self.m65_chr_banks[3] = value,
                    0x1004 => self.m65_chr_banks[4] = value,
                    0x1005 => self.m65_chr_banks[5] = value,
                    0x1006 => self.m65_chr_banks[6] = value,
                    0x1007 => self.m65_chr_banks[7] = value,
                    _ => {}
                }
            }
            _ => {
                log::warn!("Unsupported mapper: {}", self.mapper);
            }
        }
    }

    pub fn _read_chr(&self, addr: u16) -> u8 {
        if self.chr_rom.is_empty() {
            return 0;
        }
        
        match self.mapper {
            0 => {
                self.chr_rom[(addr & 0x1FFF) as usize]
            }
            _ => {
                log::warn!("Unsupported mapper: {}", self.mapper);
                0
            }
        }
    }

    pub fn _write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_rom.is_empty() {
            return;
        }
        
        match self.mapper {
            0 => {
                if self.chr_rom.len() == 0x2000 {
                    self.chr_rom[(addr & 0x1FFF) as usize] = value;
                }
            }
            _ => {
                log::warn!("Unsupported mapper: {}", self.mapper);
            }
        }
    }

    pub fn _mirror_vram_addr(&self, addr: u16) -> u16 {
        let mirrored_addr = addr & 0x2FFF;
        let table_index = (mirrored_addr - 0x2000) / 0x0400;
        
        match self._mirroring {
            Mirroring::Vertical => {
                match table_index {
                    0 | 2 => 0x2000 + (mirrored_addr & 0x03FF),
                    1 | 3 => 0x2400 + (mirrored_addr & 0x03FF),
                    _ => mirrored_addr,
                }
            }
            Mirroring::Horizontal => {
                match table_index {
                    0 | 1 => 0x2000 + (mirrored_addr & 0x03FF),
                    2 | 3 => 0x2400 + (mirrored_addr & 0x03FF),
                    _ => mirrored_addr,
                }
            }
            Mirroring::FourScreen => mirrored_addr,
            Mirroring::_SingleScreenLower => 0x2000 + (mirrored_addr & 0x03FF),
            Mirroring::_SingleScreenUpper => 0x2400 + (mirrored_addr & 0x03FF),
        }
    }
}