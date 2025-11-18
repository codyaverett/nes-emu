
pub struct Mapper5 {
    prg_rom: Vec<u8>,
    chr_rom: Vec<u8>,
    
    // ExRAM - 1KB internal RAM for extended attributes and nametables
    exram: [u8; 0x400],
    exram_mode: u8,
    
    // PRG banking
    prg_mode: u8,
    prg_banks: [u8; 5],  // Up to 5 banks depending on mode
    prg_ram_protect: [u8; 2],
    
    // CHR banking
    chr_mode: u8,
    chr_banks: [u16; 12],  // Up to 12 banks depending on mode
    upper_chr_bank_bits: u8,
    
    // Nametable control
    nametable_mapping: [u8; 4],
    fill_mode_tile: u8,
    fill_mode_attr: u8,
    
    // Split screen control
    vsplit_enabled: bool,
    vsplit_side: bool,  // false = left, true = right
    vsplit_tile: u8,
    vsplit_scroll: u8,
    vsplit_bank: u8,
    in_split_area: bool,
    
    // IRQ control
    irq_scanline: u8,
    irq_enabled: bool,
    irq_pending: bool,
    irq_in_frame: bool,
    scanline_counter: u8,
    
    // PPU monitoring
    ppu_is_rendering: bool,
    last_ppu_read: u16,
    consecutive_nametable_reads: u8,
    
    // Multiplication unit
    multiplicand_a: u8,
    multiplicand_b: u8,
}

impl Mapper5 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>) -> Self {
        let mut mapper = Mapper5 {
            prg_rom,
            chr_rom,
            exram: [0; 0x400],
            exram_mode: 0,
            
            prg_mode: 3,  // Default to mode 3
            prg_banks: [0xFF, 0xFF, 0xFF, 0xFF, 0xFF],  // Default to last bank
            prg_ram_protect: [0; 2],
            
            chr_mode: 0,
            chr_banks: [0; 12],
            upper_chr_bank_bits: 0,
            
            nametable_mapping: [0; 4],
            fill_mode_tile: 0,
            fill_mode_attr: 0,
            
            vsplit_enabled: false,
            vsplit_side: false,
            vsplit_tile: 0,
            vsplit_scroll: 0,
            vsplit_bank: 0,
            in_split_area: false,
            
            irq_scanline: 0,
            irq_enabled: false,
            irq_pending: false,
            irq_in_frame: false,
            scanline_counter: 0,
            
            ppu_is_rendering: false,
            last_ppu_read: 0,
            consecutive_nametable_reads: 0,
            
            multiplicand_a: 0,
            multiplicand_b: 0,
        };
        
        // Initialize PRG banks to point to last bank
        let last_bank = (mapper.prg_rom.len() / 0x2000 - 1) as u8;
        mapper.prg_banks[4] = last_bank;
        
        mapper
    }
    
    pub fn read_prg(&self, addr: u16) -> u8 {
        match self.prg_mode {
            0 => {
                // Mode 0: 32KB switchable
                let bank = (self.prg_banks[4] >> 2) as usize;
                let offset = bank * 0x8000 + (addr as usize);
                if offset < self.prg_rom.len() {
                    self.prg_rom[offset]
                } else {
                    0
                }
            }
            1 => {
                // Mode 1: 16KB + 16KB
                match addr {
                    0x0000..=0x3FFF => {
                        let bank = (self.prg_banks[2] >> 1) as usize;
                        let offset = bank * 0x4000 + (addr as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x4000..=0x7FFF => {
                        let bank = (self.prg_banks[4] >> 1) as usize;
                        let offset = bank * 0x4000 + ((addr - 0x4000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    _ => 0
                }
            }
            2 => {
                // Mode 2: 16KB + 8KB + 8KB
                match addr {
                    0x0000..=0x3FFF => {
                        let bank = (self.prg_banks[2] >> 1) as usize;
                        let offset = bank * 0x4000 + (addr as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x4000..=0x5FFF => {
                        let bank = self.prg_banks[3] as usize;
                        let offset = bank * 0x2000 + ((addr - 0x4000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    0x6000..=0x7FFF => {
                        let bank = self.prg_banks[4] as usize;
                        let offset = bank * 0x2000 + ((addr - 0x6000) as usize);
                        if offset < self.prg_rom.len() {
                            self.prg_rom[offset]
                        } else {
                            0
                        }
                    }
                    _ => 0
                }
            }
            3 => {
                // Mode 3: 8KB + 8KB + 8KB + 8KB
                let bank_index = (addr / 0x2000) as usize;
                let bank = self.prg_banks[bank_index + 1] as usize;
                let offset = bank * 0x2000 + ((addr & 0x1FFF) as usize);
                if offset < self.prg_rom.len() {
                    self.prg_rom[offset]
                } else {
                    0
                }
            }
            _ => 0
        }
    }
    
    pub fn write_prg(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5015 => {
                // Audio registers (not implemented)
            }
            0x5100 => {
                // PRG mode
                self.prg_mode = value & 0x03;
            }
            0x5101 => {
                // CHR mode
                self.chr_mode = value & 0x03;
            }
            0x5102 => {
                // PRG-RAM protect 1
                self.prg_ram_protect[0] = value & 0x03;
            }
            0x5103 => {
                // PRG-RAM protect 2
                self.prg_ram_protect[1] = value & 0x03;
            }
            0x5104 => {
                // ExRAM mode
                self.exram_mode = value & 0x03;
            }
            0x5105 => {
                // Nametable mapping
                self.nametable_mapping[0] = value & 0x03;
                self.nametable_mapping[1] = (value >> 2) & 0x03;
                self.nametable_mapping[2] = (value >> 4) & 0x03;
                self.nametable_mapping[3] = (value >> 6) & 0x03;
            }
            0x5106 => {
                // Fill mode tile
                self.fill_mode_tile = value;
            }
            0x5107 => {
                // Fill mode attribute
                self.fill_mode_attr = value & 0x03;
            }
            0x5113..=0x5117 => {
                // PRG banks
                let bank_index = (addr - 0x5113) as usize;
                self.prg_banks[bank_index] = value;
            }
            0x5120..=0x512B => {
                // CHR banks
                let bank_index = (addr - 0x5120) as usize;
                self.chr_banks[bank_index] = value as u16 | ((self.upper_chr_bank_bits as u16) << 8);
            }
            0x5130 => {
                // Upper CHR bank bits
                self.upper_chr_bank_bits = value & 0x03;
            }
            0x5200 => {
                // Vertical split mode
                self.vsplit_enabled = (value & 0x80) != 0;
                self.vsplit_side = (value & 0x40) != 0;
                self.vsplit_tile = value & 0x1F;
            }
            0x5201 => {
                // Vertical split scroll
                self.vsplit_scroll = value;
            }
            0x5202 => {
                // Vertical split bank
                self.vsplit_bank = value;
            }
            0x5203 => {
                // IRQ scanline
                self.irq_scanline = value;
            }
            0x5204 => {
                // IRQ enable
                self.irq_enabled = (value & 0x80) != 0;
                if self.irq_enabled && self.irq_pending {
                    // IRQ should fire
                }
            }
            0x5205 => {
                // Multiplicand A
                self.multiplicand_a = value;
            }
            0x5206 => {
                // Multiplicand B
                self.multiplicand_b = value;
            }
            0x5C00..=0x5FFF => {
                // ExRAM write
                if self.exram_mode < 3 {  // Mode 3 is read-only
                    self.exram[(addr - 0x5C00) as usize] = value;
                }
            }
            _ => {}
        }
    }
    
    pub fn read_chr(&self, addr: u16) -> u8 {
        // Handle CHR banking based on mode
        let bank_index = match self.chr_mode {
            0 => {
                // 8KB mode
                (addr / 0x2000) as usize * 8
            }
            1 => {
                // 4KB mode
                (addr / 0x1000) as usize * 4
            }
            2 => {
                // 2KB mode
                (addr / 0x800) as usize * 2
            }
            3 => {
                // 1KB mode
                (addr / 0x400) as usize
            }
            _ => 0
        };
        
        let bank_size: usize = match self.chr_mode {
            0 => 0x2000,
            1 => 0x1000,
            2 => 0x800,
            3 => 0x400,
            _ => 0x2000
        };
        
        let bank = self.chr_banks[bank_index] as usize;
        let offset = (bank * bank_size) + ((addr as usize) & (bank_size - 1));
        
        if offset < self.chr_rom.len() {
            self.chr_rom[offset]
        } else {
            0
        }
    }
    
    pub fn read_nametable(&self, addr: u16) -> u8 {
        // Handle special nametable modes
        let table = ((addr - 0x2000) / 0x400) as usize;
        match self.nametable_mapping[table] {
            0 | 1 => {
                // Use internal VRAM (handled by PPU)
                0
            }
            2 => {
                // Use ExRAM as nametable
                if self.exram_mode == 0 || self.exram_mode == 1 {
                    self.exram[(addr & 0x3FF) as usize]
                } else {
                    0
                }
            }
            3 => {
                // Fill mode
                if (addr & 0x3FF) < 0x3C0 {
                    // Tile data
                    self.fill_mode_tile
                } else {
                    // Attribute data
                    self.fill_mode_attr
                }
            }
            _ => 0
        }
    }
    
    pub fn get_multiplication_result(&self) -> u16 {
        (self.multiplicand_a as u16) * (self.multiplicand_b as u16)
    }
    
    pub fn clock_scanline(&mut self) {
        if self.ppu_is_rendering {
            if self.scanline_counter == self.irq_scanline {
                if self.irq_enabled {
                    self.irq_pending = true;
                }
            }
            self.scanline_counter = self.scanline_counter.wrapping_add(1);
        }
    }
    
    pub fn notify_ppu_state(&mut self, rendering: bool) {
        self.ppu_is_rendering = rendering;
        if !rendering {
            self.scanline_counter = 0;
            self.irq_in_frame = false;
        }
    }
    
    pub fn get_irq_pending(&self) -> bool {
        self.irq_pending
    }
    
    pub fn clear_irq(&mut self) {
        self.irq_pending = false;
    }
}