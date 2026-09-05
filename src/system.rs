use crate::apu::Apu;
use crate::cartridge::{Cartridge, Mapper, NullMapper};
use crate::input::Controller;
use crate::ppu::Ppu;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// Interrupt tests live in src/system/tests.rs (a child module, so they can
// reach the private CPU registers). Declared here rather than appended at the
// end of the file to keep the tail of `impl System` free for other work.
#[cfg(test)]
mod tests;

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
    /// Stand-in mapper handed to the PPU while no cartridge is loaded.
    null_mapper: NullMapper,
    /// Bus cycles ticked so far in the instruction being executed. Reset at
    /// the start of `cpu_step` and used to pad an instruction out to its
    /// documented cycle count (see `tick`).
    instr_cycles: u16,
    /// True while an OAM DMA ran inside the current instruction, which makes
    /// the ticked count legitimately exceed the opcode's declared count.
    dma_in_instr: bool,
    audio_sample_counter: f64,
    /// Samples produced by `tick` during the current frame while
    /// `audio_capture` is set; drained into the caller's buffer once per
    /// frame so the per-cycle path never takes a mutex.
    audio_out: Vec<f32>,
    audio_capture: bool,
    /// Total CPU cycles executed since power-on or the last
    /// `set_total_cpu_cycles` call. Unlike `cycles` (a per-frame budget)
    /// this never resets; it feeds the nestest-style trace CYC column.
    total_cycles: u64,
    /// Latched rising edge of the PPU NMI output. NMI is edge-triggered: the
    /// edge is captured here and serviced exactly once by `poll_interrupts`.
    nmi_pending: bool,
    /// Extra mapper-style contribution to the IRQ line (level), OR'd with the
    /// loaded mapper's own `irq_pending()`. Used by tests to drive the line
    /// without a cartridge that has an IRQ counter.
    pub mapper_irq: bool,
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
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
            null_mapper: NullMapper,
            instr_cycles: 0,
            dma_in_instr: false,
            audio_sample_counter: 0.0,
            audio_out: Vec::new(),
            audio_capture: false,
            total_cycles: 0,
            nmi_pending: false,
            mapper_irq: false,
        }
    }

    pub fn reset(&mut self) {
        self.cpu_a = 0;
        self.cpu_x = 0;
        self.cpu_y = 0;
        self.cpu_sp = 0xFD;
        self.cpu_status = 0x24;
        self.nmi_pending = false;
        self.mapper_irq = false;
        self.ppu.reset();
        self.apu.reset();
        self.controller1.reset();
        self.controller2.reset();
        self.instr_cycles = 0;
        self.dma_in_instr = false;

        // The reset sequence takes 7 cycles on hardware (nestest's log starts
        // at CYC:7, PPU dot 21). Two of them are the vector fetch.
        self.total_cycles = 0;
        self.cpu_pc = self.read_word(0xFFFC);
        for _ in 0..5 {
            self.tick();
        }
        log::info!("Reset CPU, PC set to: 0x{:04X}", self.cpu_pc);

        // Log first few bytes at reset vector for debugging
        if let Some(ref mut cart) = self.cartridge {
            let vec_lo = cart.mapper.cpu_read(0xFFFC);
            let vec_hi = cart.mapper.cpu_read(0xFFFD);
            log::info!(
                "Reset vector bytes: 0x{:02X} 0x{:02X} => PC: 0x{:04X}",
                vec_lo,
                vec_hi,
                (vec_hi as u16) << 8 | vec_lo as u16
            );
        }
    }

    pub fn load_cartridge(&mut self, cartridge: Cartridge) {
        // Pattern tables and mirroring are served by the cartridge mapper on
        // every PPU access, so nothing is copied into the PPU here.
        self.cartridge = Some(cartridge);
        self.reset();
    }

    /// Split borrow: the PPU and the mapper it should talk to for this access.
    fn ppu_and_mapper(&mut self) -> (&mut Ppu, &mut dyn Mapper) {
        let mapper: &mut dyn Mapper = match self.cartridge.as_mut() {
            Some(cart) => cart.mapper.as_mut(),
            None => &mut self.null_mapper,
        };
        (&mut self.ppu, mapper)
    }

    /// Advance the rest of the machine by one CPU cycle: three PPU dots, one
    /// APU step, the cycle counters and audio sampling. Every bus access
    /// calls this first, so PPU and APU registers are observed at the cycle
    /// the access really happens rather than after the instruction.
    fn tick(&mut self) {
        self.total_cycles += 1;
        self.instr_cycles += 1;
        for _ in 0..3 {
            self.ppu_step();
        }
        self.apu.step();

        if self.audio_capture {
            const CYCLES_PER_SAMPLE: f64 = 1_789_773.0 / 44_100.0;
            self.audio_sample_counter += 1.0;
            if self.audio_sample_counter >= CYCLES_PER_SAMPLE {
                self.audio_sample_counter -= CYCLES_PER_SAMPLE;
                let sample = self.apu.get_output();
                self.audio_out.push(sample);
            }
        }
    }

    fn read_byte(&mut self, addr: u16) -> u8 {
        self.tick();
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize],
            0x2000..=0x3FFF => {
                let (ppu, mapper) = self.ppu_and_mapper();
                ppu.read_register(0x2000 | (addr & 0x0007), mapper)
            }
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
            0x4020..=0xFFFF => match self.cartridge {
                Some(ref mut cart) => cart.mapper.cpu_read(addr),
                None => 0,
            },
            _ => 0,
        }
    }

    fn write_byte(&mut self, addr: u16, value: u8) {
        self.tick();
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize] = value,
            0x2000..=0x3FFF => {
                let (ppu, mapper) = self.ppu_and_mapper();
                ppu.write_register(0x2000 | (addr & 0x0007), value, mapper)
            }
            0x4000..=0x4013 | 0x4015 => self.apu.write_register(addr, value),
            0x4014 => {
                // OAM DMA. The CPU is halted while the DMA unit performs 256
                // read/write pairs through the bus, each pair two cycles, plus
                // one dummy cycle, plus one more alignment cycle when the DMA
                // starts on an odd CPU cycle. Every access ticks, so the PPU
                // and APU keep running underneath, as on hardware.
                self.dma_in_instr = true;
                let page = (value as u16) << 8;
                self.tick();
                if self.total_cycles % 2 == 1 {
                    self.tick();
                }
                for i in 0..256u16 {
                    let data = self.read_byte(page | i);
                    self.write_byte(0x2004, data);
                }
            }
            0x4016 => {
                log::trace!("CPU writing $4016: value={:02X}", value);
                self.controller1.write(value);
                // Controller 2 strobe is handled but we don't have a second controller
            }
            0x4017 => self.apu.write_register(addr, value),
            0x4020..=0xFFFF => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.mapper.cpu_write(addr, value);
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

    pub fn run_frame_with_audio(
        &mut self,
        audio_buffer: Option<&Arc<Mutex<VecDeque<f32>>>>,
    ) -> bool {
        let start_frame = self.ppu.frame;
        self.audio_capture = audio_buffer.is_some();

        // The PPU frame counter advances inside `tick`, so a frame ends at
        // the end of whichever instruction crosses the boundary.
        while self.ppu.frame == start_frame {
            self.cpu_step();
        }

        self.audio_capture = false;
        if let Some(buffer) = audio_buffer {
            let mut audio_buf = buffer.lock().unwrap();
            for sample in self.audio_out.drain(..) {
                if audio_buf.len() < 8192 {
                    // Hard limit to prevent overflow
                    audio_buf.push_back(sample);
                }
            }
        } else {
            self.audio_out.clear();
        }
        true
    }

    /// Execute one instruction (or one interrupt sequence) and return the
    /// number of CPU cycles it took. The PPU and APU are advanced from
    /// inside every bus access (see `tick`); instructions whose documented
    /// cycle count exceeds their bus accesses are padded at the end so the
    /// totals match hardware, and OAM DMA runs inline in the `$4014` write.
    fn cpu_step(&mut self) -> u16 {
        self.instr_cycles = 0;
        self.dma_in_instr = false;

        // Interrupt polling happens at the boundary between the previous
        // instruction and the next opcode fetch. Every cycle of the previous
        // instruction has already been ticked, so the PPU NMI latch and the
        // APU/mapper IRQ levels seen here are the ones at that boundary.
        let declared = match self.poll_interrupts() {
            Some(cycles) => cycles as u16,
            None => self.execute_opcode() as u16,
        };

        // DMC memory reader: the APU asks for a byte at the end of the
        // instruction. The 4-cycle CPU stall is not modelled, so the fetch
        // deliberately bypasses the ticking bus.
        if let Some(addr) = self.apu.dmc_fetch_address() {
            let byte = self.peek(addr);
            self.apu.dmc_supply_sample(byte);
        }

        debug_assert!(
            self.dma_in_instr || self.instr_cycles <= declared,
            "instruction performed {} bus accesses but declares {} cycles",
            self.instr_cycles,
            declared
        );
        while self.instr_cycles < declared {
            self.tick();
        }
        self.instr_cycles
    }

    fn execute_opcode(&mut self) -> u8 {
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

        match opcode {
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
            0xEA => 2,
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
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0xF0 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x02) != 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0x10 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x80) == 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0x30 => {
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x80) != 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            // More opcodes needed for Super Mario Bros
            0x78 => {
                // SEI
                self.cpu_status |= 0x04;
                2
            }
            0xD8 => {
                // CLD
                self.cpu_status &= !0x08;
                2
            }
            0x9A => {
                // TXS
                self.cpu_sp = self.cpu_x;
                2
            }
            0xA5 => {
                // LDA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0xBD => {
                // LDA absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xC9 => {
                // CMP immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0x29 => {
                // AND immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x86 => {
                // STX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_x);
                3
            }
            0x84 => {
                // STY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_y);
                3
            }
            0x8E => {
                // STX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_x);
                4
            }
            0x18 => {
                // CLC
                self.cpu_status &= !0x01;
                2
            }
            0x38 => {
                // SEC
                self.cpu_status |= 0x01;
                2
            }
            0xB0 => {
                // BCS
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x01) != 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0x90 => {
                // BCC
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x01) == 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0x00 => {
                // BRK: shares the $FFFE vector with IRQ but pushes P with the
                // B flag (bit 4) set so the handler can tell them apart. B is
                // only ever a property of the pushed copy; it is never set in
                // the live status register (that would leak into the P pushed
                // by an NMI arriving inside the BRK handler).
                self.push_word(self.cpu_pc.wrapping_add(1));
                self.push(self.cpu_status | 0x30);
                self.cpu_status |= 0x04;
                self.cpu_pc = self.read_word(0xFFFE);
                7
            }
            0x40 => {
                // RTI
                self.cpu_status = self.pop() & 0xEF | 0x20;
                self.cpu_pc = self.pop_word();
                6
            }
            0x48 => {
                // PHA
                self.push(self.cpu_a);
                3
            }
            0x68 => {
                // PLA
                self.cpu_a = self.pop();
                self.update_nz(self.cpu_a);
                4
            }
            0x08 => {
                // PHP
                self.push(self.cpu_status | 0x30);
                3
            }
            0x28 => {
                // PLP
                self.cpu_status = self.pop() & 0xEF | 0x20;
                4
            }
            0xAA => {
                // TAX
                self.cpu_x = self.cpu_a;
                self.update_nz(self.cpu_x);
                2
            }
            0x8A => {
                // TXA
                self.cpu_a = self.cpu_x;
                self.update_nz(self.cpu_a);
                2
            }
            0xA8 => {
                // TAY
                self.cpu_y = self.cpu_a;
                self.update_nz(self.cpu_y);
                2
            }
            0x98 => {
                // TYA
                self.cpu_a = self.cpu_y;
                self.update_nz(self.cpu_a);
                2
            }
            0xBA => {
                // TSX
                self.cpu_x = self.cpu_sp;
                self.update_nz(self.cpu_x);
                2
            }
            0x09 => {
                // ORA immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x49 => {
                // EOR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= value;
                self.update_nz(self.cpu_a);
                2
            }
            0x69 => {
                // ADC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.adc(value);
                2
            }
            0xE9 => {
                // SBC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.sbc(value);
                2
            }
            0xEB => {
                // Unofficial: SBC immediate (duplicate)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.sbc(value);
                2
            }
            0x91 => {
                // STA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                self.write_byte(addr, self.cpu_a);
                6
            }
            0x06 => {
                // ASL zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xC0 => {
                // CPY immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_y.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_y >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0xE0 => {
                // CPX immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let result = self.cpu_x.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_x >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                2
            }
            0xB1 => {
                // LDA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            0xB5 => {
                // LDA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.read_byte(addr & 0xFF);
                self.update_nz(self.cpu_a);
                4
            }
            0xB9 => {
                // LDA absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xA6 => {
                // LDX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                3
            }
            0xB4 => {
                // LDY zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                4
            }
            0xA4 => {
                // LDY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                3
            }
            0x0A => {
                // ASL A
                self.cpu_status =
                    (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                self.cpu_a <<= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x4A => {
                // LSR A
                self.cpu_status =
                    (self.cpu_status & !0x01) | if self.cpu_a & 0x01 != 0 { 0x01 } else { 0 };
                self.cpu_a >>= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x2A => {
                // ROL A
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                self.cpu_a = (self.cpu_a << 1) | carry;
                self.update_nz(self.cpu_a);
                2
            }
            0x6A => {
                // ROR A
                let carry = (self.cpu_status & 0x01) << 7;
                self.cpu_status = (self.cpu_status & !0x01) | (self.cpu_a & 0x01);
                self.cpu_a = (self.cpu_a >> 1) | carry;
                self.update_nz(self.cpu_a);
                2
            }
            0x24 => {
                // BIT zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0xC2)
                    | if self.cpu_a & value == 0 { 0x02 } else { 0 }
                    | (value & 0xC0);
                3
            }
            0x2C => {
                // BIT absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0xC2)
                    | if self.cpu_a & value == 0 { 0x02 } else { 0 }
                    | (value & 0xC0);
                4
            }
            // More addressing modes and instructions
            0xA1 => {
                // LDA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a = self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x81 => {
                // STA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.write_byte(addr, self.cpu_a);
                6
            }
            0x99 => {
                // STA absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a);
                5
            }
            0x9D => {
                // STA absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a);
                5
            }
            // ASL variants
            0x16 => {
                // ASL zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x0E => {
                // ASL absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x1E => {
                // ASL absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // LSR variants
            0x46 => {
                // LSR zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0x56 => {
                // LSR zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x4E => {
                // LSR absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status = (self.cpu_status & !0x01) | (value & 0x01);
                value >>= 1;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x5E => {
                // LSR absolute,X
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
            0x26 => {
                // ROL zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0x36 => {
                // ROL zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x2E => {
                // ROL absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0x3E => {
                // ROL absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // ROR variants
            0x66 => {
                // ROR zero page
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
            0x76 => {
                // ROR zero page,X
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
            0x6E => {
                // ROR absolute
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
            0x7E => {
                // ROR absolute,X
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
            0xE6 => {
                // INC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xF6 => {
                // INC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xEE => {
                // INC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xFE => {
                // INC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_add(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            0xC6 => {
                // DEC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                5
            }
            0xD6 => {
                // DEC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xCE => {
                // DEC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                6
            }
            0xDE => {
                // DEC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr).wrapping_sub(1);
                self.write_byte(addr, value);
                self.update_nz(value);
                7
            }
            // Additional branch instructions
            0x50 => {
                // BVC - Branch if overflow clear
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x40) == 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            0x70 => {
                // BVS - Branch if overflow set
                let offset = self.read_byte(self.cpu_pc) as i8;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                if (self.cpu_status & 0x40) != 0 {
                    self.branch_taken(offset)
                } else {
                    2
                }
            }
            // Flag instructions
            0xB8 => {
                // CLV - Clear overflow flag
                self.cpu_status &= !0x40;
                2
            }
            0x58 => {
                // CLI - Clear interrupt disable
                self.cpu_status &= !0x04;
                2
            }
            0xF8 => {
                // SED - Set decimal flag
                self.cpu_status |= 0x08;
                2
            }
            // JMP indirect
            0x6C => {
                // JMP (indirect)
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
            0xAE => {
                // LDX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                4
            }
            0xBE => {
                // LDX absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xB6 => {
                // LDX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_x = self.read_byte(addr);
                self.update_nz(self.cpu_x);
                4
            }
            0xAC => {
                // LDY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                4
            }
            0xBC => {
                // LDY absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_y = self.read_byte(addr);
                self.update_nz(self.cpu_y);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            // More STX/STY variants
            0x8C => {
                // STY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_y);
                4
            }
            0x96 => {
                // STX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_x);
                4
            }
            0x94 => {
                // STY zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_y);
                4
            }
            // More comparison instructions
            0xC5 => {
                // CMP zero page
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
            0xD5 => {
                // CMP zero page,X
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
            0xCD => {
                // CMP absolute
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
            0xDD => {
                // CMP absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xD9 => {
                // CMP absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                let result = self.cpu_a.wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x83)
                    | if self.cpu_a >= value { 0x01 } else { 0 }
                    | if result == 0 { 0x02 } else { 0 }
                    | if result & 0x80 != 0 { 0x80 } else { 0 };
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xC1 => {
                // CMP (indirect,X)
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
            0xD1 => {
                // CMP (indirect),Y
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
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // AND variants
            0x25 => {
                // AND zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x35 => {
                // AND zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x2D => {
                // AND absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x3D => {
                // AND absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x39 => {
                // AND absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x21 => {
                // AND (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x31 => {
                // AND (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a &= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // ORA variants
            0x05 => {
                // ORA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x15 => {
                // ORA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x0D => {
                // ORA absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x1D => {
                // ORA absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x19 => {
                // ORA absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x01 => {
                // ORA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x11 => {
                // ORA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a |= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // EOR variants
            0x45 => {
                // EOR zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                3
            }
            0x55 => {
                // EOR zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x4D => {
                // EOR absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                4
            }
            0x5D => {
                // EOR absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x59 => {
                // EOR absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x41 => {
                // EOR (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                6
            }
            0x51 => {
                // EOR (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                self.cpu_a ^= self.read_byte(addr);
                self.update_nz(self.cpu_a);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // ADC variants
            0x65 => {
                // ADC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.adc(value);
                3
            }
            0x75 => {
                // ADC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.adc(value);
                4
            }
            0x6D => {
                // ADC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                4
            }
            0x7D => {
                // ADC absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x79 => {
                // ADC absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0x61 => {
                // ADC (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                self.adc(value);
                6
            }
            0x71 => {
                // ADC (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.adc(value);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // SBC variants
            0xE5 => {
                // SBC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.sbc(value);
                3
            }
            0xF5 => {
                // SBC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.sbc(value);
                4
            }
            0xED => {
                // SBC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                4
            }
            0xFD => {
                // SBC absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xF9 => {
                // SBC absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xE1 => {
                // SBC (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let value = self.read_byte(addr);
                self.sbc(value);
                6
            }
            0xF1 => {
                // SBC (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.sbc(value);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // CPX/CPY variants
            0xE4 => {
                // CPX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.compare(self.cpu_x, value);
                3
            }
            0xEC => {
                // CPX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.compare(self.cpu_x, value);
                4
            }
            0xC4 => {
                // CPY zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.compare(self.cpu_y, value);
                3
            }
            0xCC => {
                // CPY absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.compare(self.cpu_y, value);
                4
            }
            // Unofficial/Illegal opcodes
            // NOPs (various addressing modes and cycle counts)
            0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => {
                // NOP implied
                2
            }
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => {
                // NOP immediate
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                2
            }
            0x04 | 0x44 | 0x64 => {
                // NOP zero page
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                3
            }
            0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => {
                // NOP zero page,X
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                4
            }
            0x0C => {
                // NOP absolute
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                4
            }
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => {
                // NOP absolute,X: reads the operand like LDA abs,X, so it
                // takes the extra cycle when the index crosses a page.
                let base = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let addr = base.wrapping_add(self.cpu_x as u16);
                let _ = self.read_byte(addr);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            // LAX - LDA + LDX
            0xA7 => {
                // LAX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                3
            }
            0xB7 => {
                // LAX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                4
            }
            0xAF => {
                // LAX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                4
            }
            0xBF => {
                // LAX absolute,Y
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                if Self::page_crossed(base, addr) {
                    5
                } else {
                    4
                }
            }
            0xA3 => {
                // LAX (indirect,X)
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
            0xB3 => {
                // LAX (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let indirect = (hi << 8) | lo;
                let addr = indirect.wrapping_add(self.cpu_y as u16);
                let value = self.read_byte(addr);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                if Self::page_crossed(indirect, addr) {
                    6
                } else {
                    5
                }
            }
            // SAX - Store A & X
            0x87 => {
                // SAX zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                3
            }
            0x97 => {
                // SAX zero page,Y
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_y) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                4
            }
            0x8F => {
                // SAX absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                4
            }
            0x83 => {
                // SAX (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                self.write_byte(addr, self.cpu_a & self.cpu_x);
                6
            }
            // DCP - DEC + CMP
            0xC7 => {
                // DCP zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                5
            }
            0xD7 => {
                // DCP zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                6
            }
            0xCF => {
                // DCP absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                6
            }
            0xDF => {
                // DCP absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                7
            }
            0xDB => {
                // DCP absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_sub(1);
                self.write_byte(addr, value);
                self.compare(self.cpu_a, value);
                7
            }
            0xC3 => {
                // DCP (indirect,X)
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
            0xD3 => {
                // DCP (indirect),Y
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
            0xE7 => {
                // ISC zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                5
            }
            0xF7 => {
                // ISC zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                6
            }
            0xEF => {
                // ISC absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                6
            }
            0xFF => {
                // ISC absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                7
            }
            0xFB => {
                // ISC absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                value = value.wrapping_add(1);
                self.write_byte(addr, value);
                self.sbc(value);
                7
            }
            0xE3 => {
                // ISC (indirect,X)
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
            0xF3 => {
                // ISC (indirect),Y
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
            0x07 => {
                // SLO zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                5
            }
            0x17 => {
                // SLO zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x0F => {
                // SLO absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x1F => {
                // SLO absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x1B => {
                // SLO absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x03 => {
                // SLO (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                8
            }
            0x13 => {
                // SLO (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value <<= 1;
                self.write_byte(addr, value);
                self.cpu_a |= value;
                self.update_nz(self.cpu_a);
                8
            }
            // RLA - ROL + AND
            0x27 => {
                // RLA zero page
                let addr = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                5
            }
            0x37 => {
                // RLA zero page,X
                let addr = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16 & 0xFF;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x2F => {
                // RLA absolute
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                6
            }
            0x3F => {
                // RLA absolute,X
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x3B => {
                // RLA absolute,Y
                let addr = self.read_word(self.cpu_pc).wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                7
            }
            0x23 => {
                // RLA (indirect,X)
                let base = self.read_byte(self.cpu_pc).wrapping_add(self.cpu_x) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base & 0xFF) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = (hi << 8) | lo;
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                8
            }
            0x33 => {
                // RLA (indirect),Y
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let mut value = self.read_byte(addr);
                let carry = self.cpu_status & 0x01;
                self.cpu_status =
                    (self.cpu_status & !0x01) | if value & 0x80 != 0 { 0x01 } else { 0 };
                value = (value << 1) | carry;
                self.write_byte(addr, value);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                8
            }
            // SRE - LSR + EOR
            0x47 => {
                // SRE zero page
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
            0x57 => {
                // SRE zero page,X
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
            0x4F => {
                // SRE absolute
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
            0x5F => {
                // SRE absolute,X
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
            0x5B => {
                // SRE absolute,Y
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
            0x43 => {
                // SRE (indirect,X)
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
            0x53 => {
                // SRE (indirect),Y
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
            0x67 => {
                // RRA zero page
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
            0x77 => {
                // RRA zero page,X
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
            0x6F => {
                // RRA absolute
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
            0x7F => {
                // RRA absolute,X
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
            0x7B => {
                // RRA absolute,Y
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
            0x63 => {
                // RRA (indirect,X)
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
            0x73 => {
                // RRA (indirect),Y
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
            0x0B | 0x2B => {
                // ANC immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if self.cpu_a & 0x80 != 0 { 0x01 } else { 0 };
                2
            }
            0x4B => {
                // ALR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.cpu_status = (self.cpu_status & !0x01) | (self.cpu_a & 0x01);
                self.cpu_a >>= 1;
                self.update_nz(self.cpu_a);
                2
            }
            0x6B => {
                // ARR immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a &= value;
                self.cpu_a = (self.cpu_a >> 1) | ((self.cpu_status & 0x01) << 7);
                self.cpu_status =
                    (self.cpu_status & !0x01) | if self.cpu_a & 0x40 != 0 { 0x01 } else { 0 };
                self.cpu_status = (self.cpu_status & !0x40)
                    | if ((self.cpu_a >> 5) & 1) ^ ((self.cpu_a >> 6) & 1) != 0 {
                        0x40
                    } else {
                        0
                    };
                self.update_nz(self.cpu_a);
                2
            }
            0xCB => {
                // AXS immediate
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let temp = (self.cpu_a & self.cpu_x).wrapping_sub(value);
                self.cpu_status = (self.cpu_status & !0x01)
                    | if (self.cpu_a & self.cpu_x) >= value {
                        0x01
                    } else {
                        0
                    };
                self.cpu_x = temp;
                self.update_nz(self.cpu_x);
                2
            }
            0x8B => {
                // XAA immediate (highly unstable)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = self.cpu_x;
                self.cpu_a &= value;
                self.update_nz(self.cpu_a);
                2
            }
            0xAB => {
                // LAX immediate (undocumented, unstable)
                let value = self.read_byte(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                self.cpu_a = value;
                self.cpu_x = value;
                self.update_nz(value);
                2
            }
            0x93 => {
                // AHX (indirect),Y (highly unstable)
                let base = self.read_byte(self.cpu_pc) as u16;
                self.cpu_pc = self.cpu_pc.wrapping_add(1);
                let lo = self.read_byte(base) as u16;
                let hi = self.read_byte((base + 1) & 0xFF) as u16;
                let addr = ((hi << 8) | lo).wrapping_add(self.cpu_y as u16);
                let value = self.cpu_a & self.cpu_x & (hi as u8).wrapping_add(1);
                self.write_byte(addr, value);
                6
            }
            0x9F => {
                // AHX absolute,Y (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_a & self.cpu_x & hi;
                self.write_byte(addr.wrapping_add(self.cpu_y as u16), value);
                5
            }
            0x9C => {
                // SHY absolute,X (highly unstable). Stores Y AND (high byte
                // of the base address + 1). When the index crosses a page the
                // stored value also replaces the high byte of the target.
                let base = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let addr = base.wrapping_add(self.cpu_x as u16);
                let value = self.cpu_y & ((base >> 8) as u8).wrapping_add(1);
                let addr = if Self::page_crossed(base, addr) {
                    ((value as u16) << 8) | (addr & 0x00FF)
                } else {
                    addr
                };
                self.write_byte(addr, value);
                5
            }
            0x9E => {
                // SHX absolute,Y (highly unstable). Same quirk as SHY with X.
                let base = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                let addr = base.wrapping_add(self.cpu_y as u16);
                let value = self.cpu_x & ((base >> 8) as u8).wrapping_add(1);
                let addr = if Self::page_crossed(base, addr) {
                    ((value as u16) << 8) | (addr & 0x00FF)
                } else {
                    addr
                };
                self.write_byte(addr, value);
                5
            }
            0x9B => {
                // TAS absolute,Y (highly unstable)
                let addr = self.read_word(self.cpu_pc);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.cpu_sp = self.cpu_a & self.cpu_x;
                let hi = ((addr >> 8) as u8).wrapping_add(1);
                let value = self.cpu_sp & hi;
                self.write_byte(addr.wrapping_add(self.cpu_y as u16), value);
                5
            }
            0xBB => {
                // LAS absolute,Y
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
        }
    }

    /// Apply a taken branch. Costs 3 cycles, or 4 when the target is on a
    /// different page from the address of the next instruction.
    fn branch_taken(&mut self, offset: i8) -> u8 {
        let next = self.cpu_pc;
        self.cpu_pc = next.wrapping_add(offset as u16);
        if Self::page_crossed(next, self.cpu_pc) {
            4
        } else {
            3
        }
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
        self.cpu_status = (self.cpu_status & !0x40)
            | if ((self.cpu_a ^ result) & (value ^ result) & 0x80) != 0 {
                0x40
            } else {
                0
            };

        self.cpu_a = result;
        self.update_nz(self.cpu_a);
    }

    fn sbc(&mut self, value: u8) {
        let sum = self.cpu_a as u16 + (!value) as u16 + (self.cpu_status & 0x01) as u16;
        let result = sum as u8;

        // Set carry flag
        self.cpu_status = (self.cpu_status & !0x01) | if sum > 0xFF { 0x01 } else { 0 };

        // Set overflow flag
        self.cpu_status = (self.cpu_status & !0x40)
            | if ((self.cpu_a ^ result) & ((!value) ^ result) & 0x80) != 0 {
                0x40
            } else {
                0
            };

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
        let (ppu, mapper) = self.ppu_and_mapper();
        ppu.step(mapper);
    }

    /// Poll the interrupt inputs at an instruction boundary and service one
    /// if due. Returns the cycles consumed (7) when an interrupt sequence ran.
    ///
    /// NMI is edge-triggered: the PPU raises `nmi_interrupt` exactly once per
    /// rising edge of its NMI output (vblank start with NMI enabled, or NMI
    /// enabled during vblank) and we consume that latch here, so each edge is
    /// serviced once no matter how long the output stays high.
    ///
    /// IRQ is level-triggered: the line is the OR of every IRQ source and is
    /// serviced whenever it is high and the I flag is clear. The sources hold
    /// their flags until acknowledged, so a handler that never clears its
    /// source is re-entered as soon as it executes RTI, as on hardware.
    ///
    /// NMI has priority over IRQ.
    fn poll_interrupts(&mut self) -> Option<u8> {
        if self.ppu.nmi_interrupt {
            self.ppu.nmi_interrupt = false;
            self.nmi_pending = true;
        }

        if self.nmi_pending {
            self.nmi_pending = false;
            self.nmi();
            return Some(7);
        }

        let mapper_irq = self.mapper_irq
            || self
                .cartridge
                .as_ref()
                .is_some_and(|cart| cart.mapper.irq_pending());
        let irq_line = self.apu.irq_pending() || mapper_irq;
        if irq_line && (self.cpu_status & 0x04) == 0 {
            self.irq();
            return Some(7);
        }

        None
    }

    fn nmi(&mut self) {
        self.push_word(self.cpu_pc);
        // Hardware interrupts push P with B (bit 4) clear and bit 5 set.
        self.push((self.cpu_status & !0x10) | 0x20);
        self.cpu_status |= 0x04; // Set interrupt disable
        self.cpu_pc = self.read_word(0xFFFA);
    }

    fn irq(&mut self) {
        self.push_word(self.cpu_pc);
        // Same vector as BRK, but B is clear so the handler can distinguish.
        self.push((self.cpu_status & !0x10) | 0x20);
        self.cpu_status |= 0x04; // Set interrupt disable
        self.cpu_pc = self.read_word(0xFFFE);
    }

    // ------------------------------------------------------------------
    // Headless test / trace support.
    //
    // Everything below is used by the integration harness in `tests/`
    // (see docs/testing/TEST_ROM_HARNESS.md). None of it is used by the
    // main emulation loop.
    // ------------------------------------------------------------------

    /// Read a byte without side effects.
    ///
    /// Covers CPU RAM ($0000-$1FFF, mirrored), cartridge PRG RAM
    /// ($6000-$7FFF) and PRG ROM ($8000-$FFFF). PPU/APU/controller
    /// registers ($2000-$5FFF) have read side effects (for example a $2002
    /// read clears the vblank flag) and are deliberately NOT touched;
    /// peeking that range returns 0.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize],
            0x6000..=0xFFFF => match self.cartridge {
                Some(ref cart) => cart.mapper.cpu_peek(addr),
                None => 0,
            },
            _ => 0,
        }
    }

    /// Side-effect-free little-endian 16-bit read built on `peek`.
    pub fn peek_word(&self, addr: u16) -> u16 {
        let lo = self.peek(addr) as u16;
        let hi = self.peek(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    /// Write a byte into CPU RAM ($0000-$1FFF) or PRG RAM ($6000-$7FFF)
    /// without touching any memory-mapped register. Other ranges ignored.
    pub fn poke(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize] = value,
            0x6000..=0x7FFF => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.mapper.cpu_write(addr, value);
                }
            }
            _ => {}
        }
    }

    pub fn pc(&self) -> u16 {
        self.cpu_pc
    }

    pub fn set_pc(&mut self, pc: u16) {
        self.cpu_pc = pc;
    }

    pub fn reg_a(&self) -> u8 {
        self.cpu_a
    }

    pub fn reg_x(&self) -> u8 {
        self.cpu_x
    }

    pub fn reg_y(&self) -> u8 {
        self.cpu_y
    }

    pub fn reg_sp(&self) -> u8 {
        self.cpu_sp
    }

    pub fn reg_p(&self) -> u8 {
        self.cpu_status
    }

    pub fn set_reg_a(&mut self, value: u8) {
        self.cpu_a = value;
    }

    pub fn set_reg_x(&mut self, value: u8) {
        self.cpu_x = value;
    }

    pub fn set_reg_y(&mut self, value: u8) {
        self.cpu_y = value;
    }

    pub fn set_reg_sp(&mut self, value: u8) {
        self.cpu_sp = value;
    }

    pub fn set_reg_p(&mut self, value: u8) {
        self.cpu_status = value;
    }

    /// Total CPU cycles executed so far (never reset by frame boundaries).
    pub fn total_cpu_cycles(&self) -> u64 {
        self.total_cycles
    }

    /// Override the running cycle count. nestest's golden log starts at
    /// CYC:7 (the cycles a real reset sequence takes), so the harness sets
    /// this before stepping.
    pub fn set_total_cpu_cycles(&mut self, cycles: u64) {
        self.total_cycles = cycles;
    }

    /// Execute exactly one CPU instruction (or interrupt sequence) and
    /// return the CPU cycles consumed. The PPU and APU advance inside the
    /// instruction's bus accesses, an OAM DMA started by a `$4014` write runs
    /// to completion inside the instruction, and NMI/IRQ are polled at the
    /// instruction boundary, exactly as in `run_frame`.
    pub fn step_instruction(&mut self) -> u32 {
        self.cpu_step() as u32
    }

    /// Length in bytes of the instruction at `addr`, derived from its
    /// addressing mode. Covers all 256 opcodes including unofficial ones.
    pub fn instruction_length(&self, addr: u16) -> u16 {
        let (_, mode, _) = OPCODE_TABLE[self.peek(addr) as usize];
        mode.length()
    }

    /// Produce a Nintendulator-format trace line for the instruction at the
    /// current PC without executing it, for example:
    ///
    /// `C000  4C F5 C5  JMP $C5F5                       A:00 X:00 Y:00 P:24 SP:FD PPU:  0, 21 CYC:7`
    ///
    /// Register and CYC columns match nestest.log exactly. The disassembly
    /// column is best-effort: mnemonic and operand are printed, but the
    /// "= value" annotations Nintendulator adds for memory operands are
    /// omitted, so compare parsed fields rather than whole lines.
    pub fn trace_line(&self) -> String {
        let pc = self.cpu_pc;
        let opcode = self.peek(pc);
        let (mnemonic, mode, unofficial) = OPCODE_TABLE[opcode as usize];
        let len = mode.length();

        let mut bytes = String::new();
        for i in 0..3u16 {
            if i < len {
                bytes.push_str(&format!("{:02X} ", self.peek(pc.wrapping_add(i))));
            } else {
                bytes.push_str("   ");
            }
        }

        let b1 = self.peek(pc.wrapping_add(1));
        let b2 = self.peek(pc.wrapping_add(2));
        let abs = ((b2 as u16) << 8) | b1 as u16;
        let operand = match mode {
            AddrMode::Imp => String::new(),
            AddrMode::Acc => "A".to_string(),
            AddrMode::Imm => format!("#${:02X}", b1),
            AddrMode::Zp => format!("${:02X}", b1),
            AddrMode::Zpx => format!("${:02X},X", b1),
            AddrMode::Zpy => format!("${:02X},Y", b1),
            AddrMode::Abs => format!("${:04X}", abs),
            AddrMode::Abx => format!("${:04X},X", abs),
            AddrMode::Aby => format!("${:04X},Y", abs),
            AddrMode::Ind => format!("(${:04X})", abs),
            AddrMode::Izx => format!("(${:02X},X)", b1),
            AddrMode::Izy => format!("(${:02X}),Y", b1),
            AddrMode::Rel => {
                let target = pc.wrapping_add(2).wrapping_add(b1 as i8 as i16 as u16);
                format!("${:04X}", target)
            }
        };
        let marker = if unofficial { '*' } else { ' ' };
        let asm = if operand.is_empty() {
            mnemonic.to_string()
        } else {
            format!("{} {}", mnemonic, operand)
        };

        format!(
            "{:04X}  {}{}{:<32}A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X} PPU:{:3},{:3} CYC:{}",
            pc,
            bytes,
            marker,
            asm,
            self.cpu_a,
            self.cpu_x,
            self.cpu_y,
            self.cpu_status,
            self.cpu_sp,
            self.ppu.scanline,
            self.ppu.cycle,
            self.total_cycles
        )
    }
}

pub use trace_tables::AddrMode;
use trace_tables::OPCODE_TABLE;

/// Opcode metadata for `System::trace_line` and
/// `System::instruction_length`. Kept in its own module so the glob import
/// of the mode names does not leak into the rest of the file.
mod trace_tables {
    /// 6502 addressing modes, used only for trace output and instruction
    /// length lookup. The CPU core itself decodes inline in `cpu_step`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AddrMode {
        Imp,
        Acc,
        Imm,
        Zp,
        Zpx,
        Zpy,
        Abs,
        Abx,
        Aby,
        Ind,
        Izx,
        Izy,
        Rel,
    }

    impl AddrMode {
        pub fn length(self) -> u16 {
            match self {
                AddrMode::Imp | AddrMode::Acc => 1,
                AddrMode::Imm
                | AddrMode::Zp
                | AddrMode::Zpx
                | AddrMode::Zpy
                | AddrMode::Izx
                | AddrMode::Izy
                | AddrMode::Rel => 2,
                AddrMode::Abs | AddrMode::Abx | AddrMode::Aby | AddrMode::Ind => 3,
            }
        }
    }

    use AddrMode::*;

    /// (mnemonic, addressing mode, is_unofficial) for every opcode.
#[rustfmt::skip]
pub const OPCODE_TABLE: [(&str, AddrMode, bool); 256] = [
    // 0x00
    ("BRK", Imp, false), ("ORA", Izx, false), ("KIL", Imp, true),  ("SLO", Izx, true),
    ("NOP", Zp,  true),  ("ORA", Zp,  false), ("ASL", Zp,  false), ("SLO", Zp,  true),
    ("PHP", Imp, false), ("ORA", Imm, false), ("ASL", Acc, false), ("ANC", Imm, true),
    ("NOP", Abs, true),  ("ORA", Abs, false), ("ASL", Abs, false), ("SLO", Abs, true),
    // 0x10
    ("BPL", Rel, false), ("ORA", Izy, false), ("KIL", Imp, true),  ("SLO", Izy, true),
    ("NOP", Zpx, true),  ("ORA", Zpx, false), ("ASL", Zpx, false), ("SLO", Zpx, true),
    ("CLC", Imp, false), ("ORA", Aby, false), ("NOP", Imp, true),  ("SLO", Aby, true),
    ("NOP", Abx, true),  ("ORA", Abx, false), ("ASL", Abx, false), ("SLO", Abx, true),
    // 0x20
    ("JSR", Abs, false), ("AND", Izx, false), ("KIL", Imp, true),  ("RLA", Izx, true),
    ("BIT", Zp,  false), ("AND", Zp,  false), ("ROL", Zp,  false), ("RLA", Zp,  true),
    ("PLP", Imp, false), ("AND", Imm, false), ("ROL", Acc, false), ("ANC", Imm, true),
    ("BIT", Abs, false), ("AND", Abs, false), ("ROL", Abs, false), ("RLA", Abs, true),
    // 0x30
    ("BMI", Rel, false), ("AND", Izy, false), ("KIL", Imp, true),  ("RLA", Izy, true),
    ("NOP", Zpx, true),  ("AND", Zpx, false), ("ROL", Zpx, false), ("RLA", Zpx, true),
    ("SEC", Imp, false), ("AND", Aby, false), ("NOP", Imp, true),  ("RLA", Aby, true),
    ("NOP", Abx, true),  ("AND", Abx, false), ("ROL", Abx, false), ("RLA", Abx, true),
    // 0x40
    ("RTI", Imp, false), ("EOR", Izx, false), ("KIL", Imp, true),  ("SRE", Izx, true),
    ("NOP", Zp,  true),  ("EOR", Zp,  false), ("LSR", Zp,  false), ("SRE", Zp,  true),
    ("PHA", Imp, false), ("EOR", Imm, false), ("LSR", Acc, false), ("ALR", Imm, true),
    ("JMP", Abs, false), ("EOR", Abs, false), ("LSR", Abs, false), ("SRE", Abs, true),
    // 0x50
    ("BVC", Rel, false), ("EOR", Izy, false), ("KIL", Imp, true),  ("SRE", Izy, true),
    ("NOP", Zpx, true),  ("EOR", Zpx, false), ("LSR", Zpx, false), ("SRE", Zpx, true),
    ("CLI", Imp, false), ("EOR", Aby, false), ("NOP", Imp, true),  ("SRE", Aby, true),
    ("NOP", Abx, true),  ("EOR", Abx, false), ("LSR", Abx, false), ("SRE", Abx, true),
    // 0x60
    ("RTS", Imp, false), ("ADC", Izx, false), ("KIL", Imp, true),  ("RRA", Izx, true),
    ("NOP", Zp,  true),  ("ADC", Zp,  false), ("ROR", Zp,  false), ("RRA", Zp,  true),
    ("PLA", Imp, false), ("ADC", Imm, false), ("ROR", Acc, false), ("ARR", Imm, true),
    ("JMP", Ind, false), ("ADC", Abs, false), ("ROR", Abs, false), ("RRA", Abs, true),
    // 0x70
    ("BVS", Rel, false), ("ADC", Izy, false), ("KIL", Imp, true),  ("RRA", Izy, true),
    ("NOP", Zpx, true),  ("ADC", Zpx, false), ("ROR", Zpx, false), ("RRA", Zpx, true),
    ("SEI", Imp, false), ("ADC", Aby, false), ("NOP", Imp, true),  ("RRA", Aby, true),
    ("NOP", Abx, true),  ("ADC", Abx, false), ("ROR", Abx, false), ("RRA", Abx, true),
    // 0x80
    ("NOP", Imm, true),  ("STA", Izx, false), ("NOP", Imm, true),  ("SAX", Izx, true),
    ("STY", Zp,  false), ("STA", Zp,  false), ("STX", Zp,  false), ("SAX", Zp,  true),
    ("DEY", Imp, false), ("NOP", Imm, true),  ("TXA", Imp, false), ("XAA", Imm, true),
    ("STY", Abs, false), ("STA", Abs, false), ("STX", Abs, false), ("SAX", Abs, true),
    // 0x90
    ("BCC", Rel, false), ("STA", Izy, false), ("KIL", Imp, true),  ("AHX", Izy, true),
    ("STY", Zpx, false), ("STA", Zpx, false), ("STX", Zpy, false), ("SAX", Zpy, true),
    ("TYA", Imp, false), ("STA", Aby, false), ("TXS", Imp, false), ("TAS", Aby, true),
    ("SHY", Abx, true),  ("STA", Abx, false), ("SHX", Aby, true),  ("AHX", Aby, true),
    // 0xA0
    ("LDY", Imm, false), ("LDA", Izx, false), ("LDX", Imm, false), ("LAX", Izx, true),
    ("LDY", Zp,  false), ("LDA", Zp,  false), ("LDX", Zp,  false), ("LAX", Zp,  true),
    ("TAY", Imp, false), ("LDA", Imm, false), ("TAX", Imp, false), ("LAX", Imm, true),
    ("LDY", Abs, false), ("LDA", Abs, false), ("LDX", Abs, false), ("LAX", Abs, true),
    // 0xB0
    ("BCS", Rel, false), ("LDA", Izy, false), ("KIL", Imp, true),  ("LAX", Izy, true),
    ("LDY", Zpx, false), ("LDA", Zpx, false), ("LDX", Zpy, false), ("LAX", Zpy, true),
    ("CLV", Imp, false), ("LDA", Aby, false), ("TSX", Imp, false), ("LAS", Aby, true),
    ("LDY", Abx, false), ("LDA", Abx, false), ("LDX", Aby, false), ("LAX", Aby, true),
    // 0xC0
    ("CPY", Imm, false), ("CMP", Izx, false), ("NOP", Imm, true),  ("DCP", Izx, true),
    ("CPY", Zp,  false), ("CMP", Zp,  false), ("DEC", Zp,  false), ("DCP", Zp,  true),
    ("INY", Imp, false), ("CMP", Imm, false), ("DEX", Imp, false), ("AXS", Imm, true),
    ("CPY", Abs, false), ("CMP", Abs, false), ("DEC", Abs, false), ("DCP", Abs, true),
    // 0xD0
    ("BNE", Rel, false), ("CMP", Izy, false), ("KIL", Imp, true),  ("DCP", Izy, true),
    ("NOP", Zpx, true),  ("CMP", Zpx, false), ("DEC", Zpx, false), ("DCP", Zpx, true),
    ("CLD", Imp, false), ("CMP", Aby, false), ("NOP", Imp, true),  ("DCP", Aby, true),
    ("NOP", Abx, true),  ("CMP", Abx, false), ("DEC", Abx, false), ("DCP", Abx, true),
    // 0xE0
    ("CPX", Imm, false), ("SBC", Izx, false), ("NOP", Imm, true),  ("ISB", Izx, true),
    ("CPX", Zp,  false), ("SBC", Zp,  false), ("INC", Zp,  false), ("ISB", Zp,  true),
    ("INX", Imp, false), ("SBC", Imm, false), ("NOP", Imp, false), ("SBC", Imm, true),
    ("CPX", Abs, false), ("SBC", Abs, false), ("INC", Abs, false), ("ISB", Abs, true),
    // 0xF0
    ("BEQ", Rel, false), ("SBC", Izy, false), ("KIL", Imp, true),  ("ISB", Izy, true),
    ("NOP", Zpx, true),  ("SBC", Zpx, false), ("INC", Zpx, false), ("ISB", Zpx, true),
    ("SED", Imp, false), ("SBC", Aby, false), ("NOP", Imp, true),  ("ISB", Aby, true),
    ("NOP", Abx, true),  ("SBC", Abx, false), ("INC", Abx, false), ("ISB", Abx, true),
];
}
