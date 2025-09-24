// MMC3 (Mapper 4) implementation
// Used by many popular games like Super Mario Bros 2 & 3, Mega Man 3-6, etc.

pub struct Mapper4 {
    prg_rom: Vec<u8>,
    chr_rom: Vec<u8>,
    prg_ram: [u8; 0x2000],
    
    // Bank registers
    bank_select: u8,
    bank_data: [u8; 8],
    
    // PRG RAM protect
    prg_ram_protect: bool,
    
    // Mirroring
    mirroring: u8,
    
    // IRQ
    irq_enabled: bool,
    irq_counter: u8,
    irq_latch: u8,
    irq_reload: bool,
    pub irq_pending: bool,
    
    // Current PRG banks
    prg_banks: [usize; 4],
    
    // Current CHR banks
    chr_banks: [usize; 8],
}

impl Mapper4 {
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>) -> Self {
        let prg_banks = [
            0,
            0x2000,
            prg_rom.len() - 0x4000,
            prg_rom.len() - 0x2000,
        ];
        
        let chr_banks = [0; 8];
        
        Self {
            prg_rom,
            chr_rom,
            prg_ram: [0; 0x2000],
            bank_select: 0,
            bank_data: [0; 8],
            prg_ram_protect: false,
            mirroring: 0,
            irq_enabled: false,
            irq_counter: 0,
            irq_latch: 0,
            irq_reload: false,
            irq_pending: false,
            prg_banks,
            chr_banks,
        }
    }
    
    pub fn read_prg(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => {
                if self.prg_ram_protect {
                    self.prg_ram[(addr & 0x1FFF) as usize]
                } else {
                    0
                }
            }
            0x2000..=0x3FFF => {
                self.prg_rom[self.prg_banks[0] + (addr as usize & 0x1FFF)]
            }
            0x4000..=0x5FFF => {
                self.prg_rom[self.prg_banks[1] + (addr as usize & 0x1FFF)]
            }
            0x6000..=0x7FFF => {
                self.prg_rom[self.prg_banks[2] + (addr as usize & 0x1FFF)]
            }
            _ => 0,
        }
    }
    
    pub fn write_prg(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => {
                if self.prg_ram_protect {
                    self.prg_ram[(addr & 0x1FFF) as usize] = value;
                }
            }
            0x2000..=0x3FFF if addr & 0x01 == 0 => {
                // Bank select ($8000-$9FFE, even)
                self.bank_select = value;
                self.update_banks();
            }
            0x2000..=0x3FFF => {
                // Bank data ($8001-$9FFF, odd)
                let bank = self.bank_select & 0x07;
                self.bank_data[bank as usize] = value;
                self.update_banks();
            }
            0x4000..=0x5FFF if addr & 0x01 == 0 => {
                // Mirroring ($A000-$BFFE, even)
                self.mirroring = value & 0x01;
            }
            0x4000..=0x5FFF => {
                // PRG RAM protect ($A001-$BFFF, odd)
                self.prg_ram_protect = (value & 0x80) != 0;
            }
            0x6000..=0x7FFF if addr & 0x01 == 0 => {
                // IRQ latch ($C000-$DFFE, even)
                self.irq_latch = value;
            }
            0x6000..=0x7FFF => {
                // IRQ reload ($C001-$DFFF, odd)
                self.irq_reload = true;
                self.irq_counter = 0;
            }
            0x8000..=0x9FFF if addr & 0x01 == 0 => {
                // IRQ disable ($E000-$FFFE, even)
                self.irq_enabled = false;
                self.irq_pending = false;
            }
            0x8000..=0x9FFF => {
                // IRQ enable ($E001-$FFFF, odd)
                self.irq_enabled = true;
            }
            _ => {}
        }
    }
    
    pub fn read_chr(&self, addr: u16) -> u8 {
        if self.chr_rom.is_empty() {
            return 0; // CHR RAM
        }
        
        let bank = (addr / 0x400) as usize;
        let offset = (addr & 0x3FF) as usize;
        let bank_addr = self.chr_banks[bank] + offset;
        
        if bank_addr < self.chr_rom.len() {
            self.chr_rom[bank_addr]
        } else {
            0
        }
    }
    
    pub fn write_chr(&mut self, _addr: u16, _value: u8) {
        // CHR ROM is read-only, but some games have CHR RAM
        // TODO: Implement CHR RAM support
    }
    
    pub fn clock_scanline(&mut self) {
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
    
    fn update_banks(&mut self) {
        let prg_mode = (self.bank_select >> 6) & 0x01;
        let chr_mode = (self.bank_select >> 7) & 0x01;
        
        // Update PRG banks
        if prg_mode == 0 {
            self.prg_banks[0] = (self.bank_data[6] as usize) * 0x2000 % self.prg_rom.len();
            self.prg_banks[1] = (self.bank_data[7] as usize) * 0x2000 % self.prg_rom.len();
            self.prg_banks[2] = self.prg_rom.len() - 0x4000;
            self.prg_banks[3] = self.prg_rom.len() - 0x2000;
        } else {
            self.prg_banks[0] = self.prg_rom.len() - 0x4000;
            self.prg_banks[1] = (self.bank_data[7] as usize) * 0x2000 % self.prg_rom.len();
            self.prg_banks[2] = (self.bank_data[6] as usize) * 0x2000 % self.prg_rom.len();
            self.prg_banks[3] = self.prg_rom.len() - 0x2000;
        }
        
        // Update CHR banks
        if !self.chr_rom.is_empty() {
            if chr_mode == 0 {
                self.chr_banks[0] = ((self.bank_data[0] & 0xFE) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[1] = ((self.bank_data[0] | 0x01) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[2] = ((self.bank_data[1] & 0xFE) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[3] = ((self.bank_data[1] | 0x01) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[4] = (self.bank_data[2] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[5] = (self.bank_data[3] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[6] = (self.bank_data[4] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[7] = (self.bank_data[5] as usize) * 0x400 % self.chr_rom.len();
            } else {
                self.chr_banks[0] = (self.bank_data[2] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[1] = (self.bank_data[3] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[2] = (self.bank_data[4] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[3] = (self.bank_data[5] as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[4] = ((self.bank_data[0] & 0xFE) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[5] = ((self.bank_data[0] | 0x01) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[6] = ((self.bank_data[1] & 0xFE) as usize) * 0x400 % self.chr_rom.len();
                self.chr_banks[7] = ((self.bank_data[1] | 0x01) as usize) * 0x400 % self.chr_rom.len();
            }
        }
    }
    
    pub fn get_mirroring(&self) -> u8 {
        self.mirroring
    }
}