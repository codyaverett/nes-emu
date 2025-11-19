use crate::cartridge::Cartridge;
use crate::input::Controller;
use crate::ppu::Ppu;
use crate::apu::Apu;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

pub struct System {
    cpu_ram: [u8; 0x800],
    cpu_a: u8,
    cpu_x: u8,
    cpu_y: u8,
    cpu_sp: u8,
    cpu_pc: u16,
    cpu_status: u8,
    pub ppu: Ppu,
    pub apu: Apu,
    pub controller1: Controller,
    pub controller2: Controller,
    pub cartridge: Option<Cartridge>,
    cycles: u64,
    oam_dma_cycles: u16,
    audio_sample_counter: f64,
}

impl System {
    pub fn new() -> Self {
        System {
            cpu_ram: [0; 0x800],
            cpu_a: 0,
            cpu_x: 0,
            cpu_y: 0,
            cpu_sp: 0xFD,
            cpu_pc: 0,
            cpu_status: 0x24,
            ppu: Ppu::new(),
            apu: Apu::new(),
            controller1: Controller::new(),
            controller2: Controller::new(),
            cartridge: None,
            cycles: 0,
            oam_dma_cycles: 0,
            audio_sample_counter: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.cpu_a = 0;
        self.cpu_x = 0;
        self.cpu_y = 0;
        self.cpu_sp = 0xFD;
        self.cpu_status = 0x24;
        self.ppu.reset();
        self.apu.reset();
        self.controller1.reset();
        self.controller2.reset();
        
        self.cpu_pc = self.read_word(0xFFFC);
        log::info!("Reset CPU, PC set to: 0x{:04X}", self.cpu_pc);
        
        // Log first few bytes at reset vector for debugging
        if let Some(ref cart) = self.cartridge {
            let vec_lo = cart.read_prg(0x7FFC);
            let vec_hi = cart.read_prg(0x7FFD);
            log::info!("Reset vector bytes: 0x{:02X} 0x{:02X} => PC: 0x{:04X}", 
                      vec_lo, vec_hi, (vec_hi as u16) << 8 | vec_lo as u16);
        }
    }

    pub fn load_cartridge(&mut self, cartridge: Cartridge) {
        // Set mirroring mode from cartridge
        self.ppu.mirroring = cartridge._mirroring;
        
        // Copy CHR ROM to PPU VRAM pattern tables if CHR ROM exists
        if !cartridge.chr_rom.is_empty() {
            for i in 0..cartridge.chr_rom.len().min(0x2000) {
                self.ppu.vram[i] = cartridge.chr_rom[i];
            }
        } else {
            // CHR RAM: Initialize pattern tables to zero (will be written by game)
            for i in 0..0x2000 {
                self.ppu.vram[i] = 0;
            }
        }
        
        self.cartridge = Some(cartridge);
        self.reset();
    }

    fn read_byte(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize],
            0x2000..=0x3FFF => self.ppu.read_register(0x2000 | (addr & 0x0007)),
            0x4000..=0x4015 => self.apu.read_register(addr),
            0x4016 => {
                let value = self.controller1.read();
                log::trace!("CPU reading $4016: value={:02X}", value);
                value
            }
            0x4017 => {
                // Controller 2 not connected, return 0
                0x00
            }
            0x6000..=0x7FFF => {
                if let Some(ref cart) = self.cartridge {
                    cart.prg_ram[(addr - 0x6000) as usize]
                } else {
                    0
                }
            }
            0x8000..=0xFFFF => {
                if let Some(ref cart) = self.cartridge {
                    cart.read_prg(addr - 0x8000)
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn write_byte(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize] = value,
            0x2000..=0x3FFF => self.ppu.write_register(0x2000 | (addr & 0x0007), value),
            0x4000..=0x4013 | 0x4015 => self.apu.write_register(addr, value),
            0x4014 => {
                // OAM DMA - Direct Memory Access to PPU OAM
                let page = (value as u16) << 8;
                
                // DMA takes 513 or 514 cycles (513 on odd CPU cycles, 514 on even)
                // For now we'll use 513 cycles
                self.oam_dma_cycles = 513;
                
                // Copy 256 bytes from CPU memory to OAM
                for i in 0..256 {
                    let data = self.read_byte(page | i);
                    self.ppu.oam_data[(self.ppu.oam_addr as usize + i as usize) & 0xFF] = data;
                }
            }
            0x4016 => {
                log::trace!("CPU writing $4016: value={:02X}", value);
                self.controller1.write(value);
                // Controller 2 strobe is handled but we don't have a second controller
            }
            0x4017 => self.apu.write_register(addr, value),
            0x6000..=0x7FFF => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.prg_ram[(addr - 0x6000) as usize] = value;
                }
            }
            0x8000..=0xFFFF => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.write_prg(addr - 0x8000, value);
                }
            }
            _ => {}
        }
    }

    fn read_word(&mut self, addr: u16) -> u16 {
        let lo = self.read_byte(addr) as u16;
        let hi = self.read_byte(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    pub fn run_frame(&mut self) -> bool {
        self.run_frame_with_audio(None)
    }
    
    pub fn run_frame_with_audio(&mut self, audio_buffer: Option<&Arc<Mutex<VecDeque<f32>>>>) -> bool {
        let target_cycles = 29780;
        let start_frame = self.ppu.frame;
        
        // For audio sampling - use persistent counter
        let cpu_clock_rate = 1789773.0;
        let audio_sample_rate = 44100.0;
        let cycles_per_sample = cpu_clock_rate / audio_sample_rate;
        
        while self.cycles < target_cycles {
            // CPU runs at 1/3 the speed of PPU
            let cpu_cycles = self.cpu_step();
            
            // PPU runs 3 times per CPU cycle
            for _ in 0..(cpu_cycles * 3) {
                self.ppu_step();
                if self.ppu.frame != start_frame {
                    // Frame completed
                    self.cycles = 0;
                    return true;
                }
            }
            
            // Handle NMI
            if self.ppu.nmi_interrupt {
                self.ppu.nmi_interrupt = false;
                self.nmi();
            }
            
            self.apu.step();
            
            // Generate audio samples if buffer is provided
            if let Some(buffer) = audio_buffer {
                self.audio_sample_counter += cpu_cycles as f64;

                // Check buffer size for synchronization
                let buffer_len = buffer.lock().unwrap().len();

                // Adjust sample generation based on buffer fill level
                // If buffer is too full (>6000), skip some samples to prevent overflow
                // If buffer is too empty (<2000), generate extra samples to prevent underflow
                let should_generate = if buffer_len > 6000 {
                    // Buffer getting too full, slow down sample generation
                    self.audio_sample_counter >= cycles_per_sample * 1.2
                } else if buffer_len < 2000 {
                    // Buffer running low, speed up sample generation
                    self.audio_sample_counter >= cycles_per_sample * 0.8
                } else {
                    // Normal operation
                    self.audio_sample_counter >= cycles_per_sample
                };

                while should_generate {
                    self.audio_sample_counter -= cycles_per_sample;
                    let sample = self.apu.get_output();

                    let mut audio_buf = buffer.lock().unwrap();
                    if audio_buf.len() < 8192 {  // Hard limit to prevent overflow
                        audio_buf.push_back(sample);
                    }

                    // Re-check condition for next iteration
                    if !((buffer_len > 6000 && self.audio_sample_counter >= cycles_per_sample * 1.2) ||
                         (buffer_len < 2000 && self.audio_sample_counter >= cycles_per_sample * 0.8) ||
                         (buffer_len >= 2000 && buffer_len <= 6000 && self.audio_sample_counter >= cycles_per_sample)) {
                        break;
                    }
                }
            }
            
            self.cycles += cpu_cycles as u64;
        }
        
        self.cycles -= target_cycles;
        self.ppu.frame != start_frame
    }

    fn cpu_step(&mut self) -> u8 {
        // Handle OAM DMA cycles
        if self.oam_dma_cycles > 0 {
            let cycles = self.oam_dma_cycles.min(4) as u8;
            self.oam_dma_cycles -= cycles as u16;
            return cycles;
        }
        
        let opcode = self.read_byte(self.cpu_pc);
        let old_pc = self.cpu_pc;
        self.cpu_pc = self.cpu_pc.wrapping_add(1);
        
        // Log first few instructions for debugging
        static mut INSTRUCTION_COUNT: u32 = 0;
        unsafe {
            if INSTRUCTION_COUNT < 100 {
                log::debug!("PC: 0x{:04X}, Op: 0x{:02X}", old_pc, opcode);
            }
            INSTRUCTION_COUNT += 1;
        }
        
        let cycles = match opcode {
            0xA9 => {
                self.cpu_a = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.update_nz(self.cpu_a);
                2
            }
            0xA2 => {
                self.cpu_x = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.update_nz(self.cpu_x);
                2
            }
            0xA0 => {
                self.cpu_y = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.update_nz(self.cpu_y);
                2
            }
            0x85 => {
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a);
                3
            }
            0x95 => {
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a);
                4
            }
            0x8D => {
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a);
                4
            }
            0xAD => {
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x4C => {
                self.cpu_pc = self.read_word(self.cpu_pc);
                3
            }
            0xEA => {
                2
            }
            0x20 => {
                let target = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.push_word(self.cpu_pc.wrapping_sub(1));
                self.cpu_pc = target;
                6
            }
            0x60 => {
                self.cpu_pc = self.pop_word().wrapping_add(1);
                6
            }
            0xE8 => {
                self.cpu_x = self.cpu_x.wrapping_add(1);
                self.update_nz(self.cpu_x);
                2
            }
            0xC8 => {
                self.cpu_y = self.cpu_y.wrapping_add(1);
                self.update_nz(self.cpu_y);
                2
            }
            0xCA => {
                self.cpu_x = self.cpu_x.wrapping_sub(1);
                self.update_nz(self.cpu_x);
                2
            }
            0x88 => {
                self.cpu_y = self.cpu_y.wrapping_sub(1);
                self.update_nz(self.cpu_y);
                2
            }
            0xD0 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x02) == 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0xF0 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x02) != 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0x10 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x80) == 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0x30 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x80) != 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            // More opcodes needed for Super Mario Bros
            0x78 => { // SEI
                self.cpu_status |= 0x04;
                2
            }
            0xD8 => { // CLD
                self.cpu_status &= !0x08;
                2
            }
            0x9A => { // TXS
                self.cpu_sp = self.cpu_x;
                2
            }
            0xA5 => { // LDA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0xBD => { // LDA absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xC9 => { // CMP immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0x29 => { // AND immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x86 => { // STX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_x);
                3
            }
            0x84 => { // STY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_y);
                3
            }
            0x8E => { // STX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_x);
                4
            }
            0x18 => { // CLC
                self.cpu_status &= !0x01;
                2
            }
            0x38 => { // SEC
                self.cpu_status |= 0x01;
                2
            }
            0xB0 => { // BCS
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x01) != 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0x90 => { // BCC
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x01) == 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0x00 => { // BRK
                self.cpu_status |= 0x10;
                self.push_word(self.cpu_pc.wrapping_add(1));
                self.push(self.cpu_status | 0x30);
                self.cpu_status |= 0x04;
                self.cpu_pc = self.read_word(0xFFFE);
                7
            }
            0x40 => { // RTI
                self.cpu_status = self.pop() & 0xEF | 0x20;
                self.cpu_pc = self.pop_word();
                6
            }
            0x48 => { // PHA
                self.push(self.cpu_a);
                3
            }
            0x68 => { // PLA
                self.cpu_a = self.pop();
                self.update_nz(self.cpu_a);
                4
            }
            0x08 => { // PHP
                self.push(self.cpu_status | 0x30);
                3
            }
            0x28 => { // PLP
                self.cpu_status = self.pop() & 0xEF | 0x20;
                4
            }
            0xAA => { // TAX
                self.cpu_x = self.cpu_a;
                self.update_nz(self.cpu_x);
                2
            }
            0x8A => { // TXA
                self.cpu_a = self.cpu_x;
                self.update_nz(self.cpu_a);
                2
            }
            0xA8 => { // TAY
                self.cpu_y = self.cpu_a;
                self.update_nz(self.cpu_y);
                2
            }
            0x98 => { // TYA
                self.cpu_a = self.cpu_y;
                self.update_nz(self.cpu_a);
                2
            }
            0xBA => { // TSX
                self.cpu_x = self.cpu_sp;
                self.update_nz(self.cpu_x);
                2
            }
            0x09 => { // ORA immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x49 => { // EOR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x69 => { // ADC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.adc(value);
                2
            }
            0xE9 => { // SBC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.sbc(value);
                2
            }
            0xEB => { // Unofficial: SBC immediate (duplicate)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.sbc(value);
                2
            }
            0x91 => { // STA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                self.write_byte(addr, self.cpu_a);
                6
            }
            0x06 => { // ASL zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xC0 => { // CPY immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_y.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_y >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0xE0 => { // CPX immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_x.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_x >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0xB1 => { // LDA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            0xB5 => { // LDA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.read_byte(addr & 0xFF);
                self.update_nz(self.cpu_a);
                4
            }
            0xB9 => { // LDA absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xA6 => { // LDX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                3
            }
            0xA4 => { // LDY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                3
            }
            0x0A => { // ASL A
                self.cpu_status = (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                self.cpu_a <<= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x4A => { // LSR A
                self.cpu_status = (self.cpu_status & !0x01) | if self.cpu_a & 0x01 != 0 { 0x01 } else { 0 };
                self.cpu_a >>= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x2A => { // ROL A
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                self.cpu_a = (self.cpu_a << 1) | carry;
                self.update_nz(self.cpu_a);
                2
            }
            0x6A => { // ROR A
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (self.cpu_a & 0x01);
                self.cpu_a = (self.cpu_a >> 1) | carry;
                self.update_nz(self.cpu_a);
                2
            }
            0x24 => { // BIT zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0xC2)
                    | if self.cpu_a & value == 0 { 0x02 } else { 0 }
                    | (value & 0xC0);
                3
            }
            0x2C => { // BIT absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0xC2)
                    | if self.cpu_a & value == 0 { 0x02 } else { 0 }
                    | (value & 0xC0);
                4
            }
            // More addressing modes and instructions
            0xA1 => { // LDA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x81 => { // STA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.write_byte(addr, self.cpu_a);
                6
            }
            0x99 => { // STA absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a);
                5
            }
            0x9D => { // STA absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a);
                5
            }
            // ASL variants
            0x16 => { // ASL zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x0E => { // ASL absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x1E => { // ASL absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // LSR variants
            0x46 => { // LSR zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0x56 => { // LSR zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x4E => { // LSR absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x5E => { // LSR absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // ROL variants
            0x26 => { // ROL zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0x36 => { // ROL zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x2E => { // ROL absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x3E => { // ROL absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // ROR variants
            0x66 => { // ROR zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0x76 => { // ROR zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x6E => { // ROR absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x7E => { // ROR absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // INC/DEC memory instructions
            0xE6 => { // INC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xF6 => { // INC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xEE => { // INC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xFE => { // INC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            0xC6 => { // DEC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xD6 => { // DEC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xCE => { // DEC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xDE => { // DEC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // Additional branch instructions
            0x50 => { // BVC - Branch if overflow clear
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x40) == 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            0x70 => { // BVS - Branch if overflow set
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x40) != 0 {
                    self.cpu_pc = self.cpu_pc.wrapping_add(offset as u16);
                    3
                } else {
                    2
                }
            }
            // Flag instructions
            0xB8 => { // CLV - Clear overflow flag
                self.cpu_status &= !0x40;
                2
            }
            0x58 => { // CLI - Clear interrupt disable
                self.cpu_status &= !0x04;
                2
            }
            0xF8 => { // SED - Set decimal flag
                self.cpu_status |= 0x08;
                2
            }
            // JMP indirect
            0x6C => { // JMP (indirect)
                let ptr = self.read_word(self.cpu_pc);
                // 6502 bug: doesn't cross page boundary correctly
                let lo = self.read_byte(ptr) as u16;
                let hi = if (ptr & 0xFF) == 0xFF {
                    self.read_byte(ptr & 0xFF00) as u16
                } else {
                    self.read_byte(ptr + 1) as u16
                };
                self.cpu_pc = (hi << 8) | lo;
                5
            }
            // More LDX/LDY variants
            0xAE => { // LDX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                4
            }
            0xBE => { // LDX absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xB6 => { // LDX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                4
            }
            0xAC => { // LDY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                4
            }
            0xBC => { // LDY absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            // More STX/STY variants
            0x8C => { // STY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_y);
                4
            }
            0x96 => { // STX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_x);
                4
            }
            0x94 => { // STY zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_y);
                4
            }
            // More comparison instructions
            0xC5 => { // CMP zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                3
            }
            0xD5 => { // CMP zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                4
            }
            0xCD => { // CMP absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                4
            }
            0xDD => { // CMP absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xD9 => { // CMP absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xC1 => { // CMP (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                6
            }
            0xD1 => { // CMP (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // AND variants
            0x25 => { // AND zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x35 => { // AND zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x2D => { // AND absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x3D => { // AND absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x39 => { // AND absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x21 => { // AND (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x31 => { // AND (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // ORA variants
            0x05 => { // ORA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x15 => { // ORA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x0D => { // ORA absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x1D => { // ORA absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x19 => { // ORA absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x01 => { // ORA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x11 => { // ORA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // EOR variants
            0x45 => { // EOR zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x55 => { // EOR zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x4D => { // EOR absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x5D => { // EOR absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x59 => { // EOR absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x41 => { // EOR (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x51 => { // EOR (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // ADC variants
            0x65 => { // ADC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.adc(value);
                3
            }
            0x75 => { // ADC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.adc(value);
                4
            }
            0x6D => { // ADC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                4
            }
            0x7D => { // ADC absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x79 => { // ADC absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0x61 => { // ADC (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                self.adc(value);
                6
            }
            0x71 => { // ADC (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // SBC variants
            0xE5 => { // SBC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.sbc(value);
                3
            }
            0xF5 => { // SBC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.sbc(value);
                4
            }
            0xED => { // SBC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                4
            }
            0xFD => { // SBC absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xF9 => { // SBC absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(base, addr) { 5 } else { 4 }
            }
            0xE1 => { // SBC (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                self.sbc(value);
                6
            }
            0xF1 => { // SBC (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(indirect, addr) { 6 } else { 5 }
            }
            // CPX/CPY variants
            0xE4 => { // CPX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.compare(self.cpu_x, value);
                3
            }
            0xEC => { // CPX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.compare(self.cpu_x, value);
                4
            }
            0xC4 => { // CPY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.compare(self.cpu_y, value);
                3
            }
            0xCC => { // CPY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.compare(self.cpu_y, value);
                4
            }
            // Unofficial/Illegal opcodes
            // NOPs (various addressing modes and cycle counts)
            0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => { // NOP implied
                2
            }
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => { // NOP immediate
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                2
            }
            0x04 | 0x44 | 0x64 => { // NOP zero page
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                3
            }
            0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => { // NOP zero page,X
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                4
            }
            0x0C => { // NOP absolute
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                4
            }
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => { // NOP absolute,X
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                4
            }
            // LAX - LDA + LDX
            0xA7 => { // LAX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                3
            }
            0xB7 => { // LAX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                4
            }
            0xAF => { // LAX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                4
            }
            0xBF => { // LAX absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                4
            }
            0xA3 => { // LAX (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                6
            }
            0xB3 => { // LAX (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                5
            }
            // SAX - Store A & X
            0x87 => { // SAX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                3
            }
            0x97 => { // SAX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                4
            }
            0x8F => { // SAX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                4
            }
            0x83 => { // SAX (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                6
            }
            // DCP - DEC + CMP
            0xC7 => { // DCP zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                5
            }
            0xD7 => { // DCP zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                6
            }
            0xCF => { // DCP absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                6
            }
            0xDF => { // DCP absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                7
            }
            0xDB => { // DCP absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                7
            }
            0xC3 => { // DCP (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                8
            }
            0xD3 => { // DCP (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                8
            }
            // ISC/ISB - INC + SBC
            0xE7 => { // ISC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                5
            }
            0xF7 => { // ISC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                6
            }
            0xEF => { // ISC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                6
            }
            0xFF => { // ISC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                7
            }
            0xFB => { // ISC absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                7
            }
            0xE3 => { // ISC (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                8
            }
            0xF3 => { // ISC (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                8
            }
            // SLO/ASO - ASL + ORA
            0x07 => { // SLO zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                5
            }
            0x17 => { // SLO zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x0F => { // SLO absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x1F => { // SLO absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x1B => { // SLO absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x03 => { // SLO (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                8
            }
            0x13 => { // SLO (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                8
            }
            // RLA - ROL + AND
            0x27 => { // RLA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                5
            }
            0x37 => { // RLA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x2F => { // RLA absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x3F => { // RLA absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x3B => { // RLA absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x23 => { // RLA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                8
            }
            0x33 => { // RLA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status = (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                8
            }
            // SRE - LSR + EOR
            0x47 => { // SRE zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                5
            }
            0x57 => { // SRE zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x4F => { // SRE absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x5F => { // SRE absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x5B => { // SRE absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x43 => { // SRE (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                8
            }
            0x53 => { // SRE (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                8
            }
            // RRA - ROR + ADC
            0x67 => { // RRA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                5
            }
            0x77 => { // RRA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                6
            }
            0x6F => { // RRA absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                6
            }
            0x7F => { // RRA absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                7
            }
            0x7B => { // RRA absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                7
            }
            0x63 => { // RRA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                8
            }
            0x73 => { // RRA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value = (value >> 1) | carry;
                self.write_byte(addr, value);
                self.adc(value);
                8
            }
            // Miscellaneous unofficial opcodes
            0x0B | 0x2B => { // ANC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                self.cpu_status = (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                2
            }
            0x4B => { // ALR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.cpu_status = (self.cpu_status & !0x01) | (self.cpu_a & 0x01);
                self.cpu_a >>= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x6B => { // ARR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.cpu_a = (self.cpu_a >> 1) | ((self.cpu_status & 0x01) << 7);
                self.cpu_status = (self.cpu_status & !0x01) | if self.cpu_a & 0x40 != 0 { 0x01 } else { 0 };
                self.cpu_status = (self.cpu_status & !0x40) | 
                    if ((self.cpu_a >> 5) & 1) ^ ((self.cpu_a >> 6) & 1) != 0 { 0x40 } else { 0 };
                self.update_nz(self.cpu_a);
                2
            }
            0xCB => { // AXS immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let temp = (self.cpu_a & self.cpu_x).wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x01) | if (self.cpu_a & self.cpu_x) >= value { 0x01 } else { 0 };
                self.cpu_x = temp;
                self.update_nz(self.cpu_x);
                2
            }
            0x8B => { // XAA immediate (highly unstable)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.cpu_x;
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                2
            }
            0xAB => { // LAX immediate (undocumented, unstable)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                2
            }
            0x93 => { // AHX (indirect),Y (highly unstable)
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let value = self.cpu_a & self.cpu_x & (hi as u8).wrapping_add(1);
                self.write_byte(addr, value);
                6
            }
            0x9F => { // AHX absolute,Y (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_a & self.cpu_x & hi;
                self.write_byte(addr.wrapping_add(self.cpu_y as u16), value);
                5
            }
            0x9C => { // SHY absolute,X (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_y & hi;
                self.write_byte(addr.wrapping_add(self.cpu_x as u16), value);
                5
            }
            0x9E => { // SHX absolute,Y (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_x & hi;
                self.write_byte(addr.wrapping_add(self.cpu_y as u16), value);
                5
            }
            0x9B => { // TAS absolute,Y (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_sp = self.cpu_a & self.cpu_x;
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_sp & hi;
                self.write_byte(addr.wrapping_add(self.cpu_y as u16), value);
                5
            }
            0xBB => { // LAS absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr) & self.cpu_sp;
                self.cpu_a = value;
                self.cpu_x = value;
                self.cpu_sp = value;
                self.update_nz(value);
                4
            }
            // KIL/JAM - Halt CPU
            0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 => {
                // Halt CPU - just loop forever
                self.cpu_pc = self.cpu_pc.wrapping_sub(1);
                2
            }
            _ => {
                log::debug!("Unimplemented opcode: 0x{:02X} at PC: 0x{:04X}", opcode, self.cpu_pc.wrapping_sub(1));
                2
            }
        };
        cycles
    }

    fn update_nz(&mut self, value: u8) {
        self.cpu_status = (self.cpu_status & !0x82) 
            | if value == 0 { 0x02 } else { 0 }
            | if value & 0x80 != 0 { 0x80 } else { 0 };
    }
    
    fn page_crossed(addr1: u16, addr2: u16) -> bool {
        (addr1 & 0xFF00) != (addr2 & 0xFF00)
    }
    
    fn adc(&mut self, value: u8) {
        let sum = self.cpu_a as u16 + value as u16 + (self.cpu_status & 0x01) as u16;
        let result = sum as u8;
        
        // Set carry flag
        self.cpu_status = (self.cpu_status & !0x01) | if sum > 0xFF { 0x01 } else { 0 };
        
        // Set overflow flag
        self.cpu_status = (self.cpu_status & !0x40) | 
            if ((self.cpu_a ^ result) & (value ^ result) & 0x80) != 0 { 0x40 } else { 0 };
        
        self.cpu_a = result;
        self.update_nz(self.cpu_a);
    }
    
    fn sbc(&mut self, value: u8) {
        let sum = self.cpu_a as u16 + (!value) as u16 + (self.cpu_status & 0x01) as u16;
        let result = sum as u8;
        
        // Set carry flag
        self.cpu_status = (self.cpu_status & !0x01) | if sum > 0xFF { 0x01 } else { 0 };
        
        // Set overflow flag
        self.cpu_status = (self.cpu_status & !0x40) | 
            if ((self.cpu_a ^ result) & ((!value) ^ result) & 0x80) != 0 { 0x40 } else { 0 };
        
        self.cpu_a = result;
        self.update_nz(self.cpu_a);
    }
    
    fn compare(&mut self, reg: u8, value: u8) {
        let result = reg.wrapping_sub(value);
        self.cpu_status = (self.cpu_status & !0x83)
            | if reg >= value { 0x01 } else { 0 }
            | if result == 0 { 0x02 } else { 0 }
            | if result & 0x80 != 0 { 0x80 } else { 0 };
    }

    fn push(&mut self, value: u8) {
        self.write_byte(0x0100 | self.cpu_sp as u16, value);
        self.cpu_sp = self.cpu_sp.wrapping_sub(1);
    }

    fn pop(&mut self) -> u8 {
        self.cpu_sp = self.cpu_sp.wrapping_add(1);
        self.read_byte(0x0100 | self.cpu_sp as u16)
    }

    fn push_word(&mut self, value: u16) {
        self.push((value >> 8) as u8);
        self.push(value as u8);
    }

    fn pop_word(&mut self) -> u16 {
        let lo = self.pop() as u16;
        let hi = self.pop() as u16;
        (hi << 8) | lo
    }

    pub fn get_frame_buffer(&self) -> &[u8] {
        self.ppu.get_frame_buffer()
    }

    fn ppu_step(&mut self) {
        self.ppu.step();
    }

    fn nmi(&mut self) {
        self.push_word(self.cpu_pc);
        self.push(self.cpu_status | 0x20);
        self.cpu_status |= 0x04; // Set interrupt disable
        self.cpu_pc = self.read_word(0xFFFA);
    }
}