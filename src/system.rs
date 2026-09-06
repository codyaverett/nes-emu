use crate::apu::Apu;
use crate::cartridge::{Cartridge, Mapper, NullMapper};
use crate::input::Controller;
use crate::ppu::Ppu;
use std::cell::Cell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::Path;
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
    /// Cycles that OAM or DMC DMA inserted into the current instruction.
    /// The instruction is padded to its declared count plus this, so an
    /// opcode whose padded (non-access) cycles come after the halt does
    /// not lose them to the stall.
    dma_cycles: u16,
    /// Outstanding DMC DMA request (see `DmcDma` and
    /// docs/debugging/DMC_DMA.md). Raised when the APU's sample buffer
    /// needs a byte, serviced by `read_byte` on the next CPU read cycle at
    /// or after its attempt tick, or inside OAM DMA.
    dmc_dma: Option<DmcDma>,
    /// True while the instruction being executed is a read-modify-write,
    /// whose padded cycle stands for the 6502's dummy write of the
    /// unmodified value. A DMC DMA cannot halt the CPU on a write, so the
    /// padding loop does not offer that cycle to the DMA unit. Every other
    /// padded cycle is a read on hardware (dummy or internal).
    padding_is_write: bool,
    /// Tick (1-based `instr_cycles`) of the current instruction at which a
    /// DMC DMA halted the CPU, and the number of cycles the stall inserted.
    /// The interrupt sample tick moves later by that many cycles when the
    /// halt landed at or before it (see `take_interrupt_snapshot`).
    dmc_stall: (u16, u16),
    audio_sample_counter: f64,
    /// Samples produced by `tick` during the current frame while
    /// `audio_capture` is set; drained into the caller's buffer once per
    /// frame so the per-cycle path never takes a mutex.
    audio_out: Vec<f32>,
    audio_capture: bool,
    /// State of the two first-order high-pass filters (90 Hz and 440 Hz)
    /// applied to APU output, as on the NES/Famicom output stage. They
    /// remove the mixer's DC offset so silence is 0.0 and gaps do not pop.
    audio_hp: [(f32, f32); 2],
    /// Running sum and count of mixer output over the current sample
    /// period. Averaging every CPU cycle into each 44.1 kHz sample is a box
    /// low-pass that removes the aliasing point-sampling produces.
    audio_acc: (f32, u32),
    /// Total CPU cycles executed since power-on or the last
    /// `set_total_cpu_cycles` call. Unlike `cycles` (a per-frame budget)
    /// this never resets; it feeds the nestest-style trace CYC column.
    total_cycles: u64,
    /// Latched rising edge of the PPU NMI output (the CPU's edge detector).
    /// NMI is edge-triggered: the edge is captured here in `tick` and
    /// serviced exactly once by `poll_interrupts`.
    nmi_pending: bool,
    /// Tick of the current instruction (1-based) at which `nmi_pending` was
    /// raised; 0 when it was already pending when the instruction began.
    /// The boundary poll only takes the NMI if this is at or before the
    /// instruction's sample tick (see `sample_tick`).
    nmi_seen_tick: u16,
    /// IRQ line level captured at each tick of the current instruction,
    /// bit `k - 1` for tick `k` (ticks past 16 are not recorded; the sample
    /// tick is never later than 7). See `tick`.
    irq_hist: u16,
    /// Sample tick override for the current instruction. 0 means the
    /// default, `declared - 1` (the penultimate cycle). A taken branch that
    /// does not cross a page sets it to 1.
    poll_tick: u16,
    /// Set by BRK and the interrupt sequences: these do not poll at their
    /// end, so at least one handler instruction always runs first.
    no_poll: bool,
    /// Set by CLI, SEI and PLP to the I flag as it was before the
    /// instruction. The IRQ poll at the end of those instructions uses the
    /// old value, so a change to I is only seen one instruction later.
    i_flag_for_poll: Option<bool>,
    /// Result of the previous instruction's interrupt poll, consumed by
    /// `poll_interrupts` at the start of the next `cpu_step`.
    sampled_nmi: bool,
    sampled_irq: bool,
    /// Extra mapper-style contribution to the IRQ line (level), OR'd with the
    /// loaded mapper's own `irq_pending()`. Used by tests to drive the line
    /// without a cartridge that has an IRQ counter.
    pub mapper_irq: bool,
    /// Hash of PRG RAM as last loaded from or written to the battery save
    /// file, so a periodic `save_battery` can skip the write when nothing
    /// changed. `None` until the first load or save. A `Cell` so
    /// `save_battery` can take `&self` (docs/debugging/BATTERY_SAVES.md).
    battery_saved_hash: Cell<Option<u64>>,
}

/// A scheduled DMC DMA. The APU's memory reader asks for a byte either
/// because `$4015` just enabled the channel with an empty sample buffer
/// (a "load" DMA) or because the output unit emptied the buffer (a
/// "reload" DMA). The DMA unit tries to halt the CPU from `attempt`
/// onwards; a halt only succeeds on a CPU read cycle (or on any cycle while
/// OAM DMA already holds the CPU). Once halted the unit spends a dummy
/// cycle, an alignment cycle if the following cycle is not a get, and then
/// reads the sample byte on a get cycle. nesdev "DMA".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DmcDma {
    /// First `total_cycles` value at which the halt may be attempted. A
    /// load DMA targets the get cycle 3 or 4 cycles after the `$4015`
    /// write; a reload DMA targets the first put cycle after the buffer
    /// emptied.
    attempt: u64,
    /// `total_cycles` of the successful halt cycle, once it has happened
    /// (only meaningful inside OAM DMA, where the DMC get is deferred to
    /// the next free get cycle).
    halted_at: Option<u64>,
}

/// Opcodes whose padded cycle (see `cpu_step`) is the 6502's dummy write
/// of the unmodified value: ASL, ROL, LSR, ROR, INC, DEC and the
/// unofficial SLO, RLA, SRE, RRA, DCP, ISC in their memory addressing modes.
fn is_read_modify_write(opcode: u8) -> bool {
    matches!(
        opcode,
        0x06 | 0x16 | 0x0E | 0x1E   // ASL
        | 0x26 | 0x36 | 0x2E | 0x3E // ROL
        | 0x46 | 0x56 | 0x4E | 0x5E // LSR
        | 0x66 | 0x76 | 0x6E | 0x7E // ROR
        | 0xE6 | 0xF6 | 0xEE | 0xFE // INC
        | 0xC6 | 0xD6 | 0xCE | 0xDE // DEC
        | 0x03 | 0x07 | 0x0F | 0x13 | 0x17 | 0x1B | 0x1F // SLO
        | 0x23 | 0x27 | 0x2F | 0x33 | 0x37 | 0x3B | 0x3F // RLA
        | 0x43 | 0x47 | 0x4F | 0x53 | 0x57 | 0x5B | 0x5F // SRE
        | 0x63 | 0x67 | 0x6F | 0x73 | 0x77 | 0x7B | 0x7F // RRA
        | 0xC3 | 0xC7 | 0xCF | 0xD3 | 0xD7 | 0xDB | 0xDF // DCP
        | 0xE3 | 0xE7 | 0xEF | 0xF3 | 0xF7 | 0xFB | 0xFF // ISC
    )
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
            battery_saved_hash: Cell::new(None),
            instr_cycles: 0,
            dma_cycles: 0,
            dmc_dma: None,
            dmc_stall: (0, 0),
            padding_is_write: false,
            audio_sample_counter: 0.0,
            audio_out: Vec::new(),
            audio_capture: false,
            audio_hp: [(0.0, 0.0); 2],
            audio_acc: (0.0, 0),
            total_cycles: 0,
            nmi_pending: false,
            nmi_seen_tick: 0,
            irq_hist: 0,
            poll_tick: 0,
            no_poll: false,
            i_flag_for_poll: None,
            sampled_nmi: false,
            sampled_irq: false,
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
        self.dma_cycles = 0;
        self.dmc_dma = None;
        self.dmc_stall = (0, 0);

        // The reset sequence takes 7 cycles on hardware (nestest's log starts
        // at CYC:7, PPU dot 21). Two of them are the vector fetch.
        self.total_cycles = 0;
        self.cpu_pc = self.read_word(0xFFFC);
        for _ in 0..5 {
            self.tick();
        }
        // Nothing sampled during the reset sequence is taken: the first
        // instruction after reset always runs.
        self.nmi_pending = false;
        self.irq_hist = 0;
        self.sampled_nmi = false;
        self.sampled_irq = false;
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
        self.battery_saved_hash.set(None);
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
    ///
    /// After the PPU and APU have advanced, and before the access that
    /// follows this tick, the interrupt inputs are captured: a PPU NMI edge
    /// is latched into `nmi_pending` (recording the tick it was first seen)
    /// and the IRQ line level is recorded in `irq_hist`. The poll at the end
    /// of the instruction reads back the capture from its sample tick
    /// (`sample_tick`), which is how the hardware's one-cycle detector
    /// latency is modelled: an input asserted during the last cycle of an
    /// instruction is not seen until the next instruction's poll.
    ///
    /// Sampling before the access matters because of padding: for a
    /// read-modify-write or an indexed store the real access happens one
    /// tick earlier than on hardware (the dummy access is padded at the
    /// end), so sampling at tick `declared - 1` before its access sees the
    /// same pre-access state hardware sees at its penultimate cycle.
    fn tick(&mut self) {
        self.total_cycles += 1;
        self.instr_cycles += 1;
        // The CPU samples its NMI input late in each cycle, which on the
        // PPU side lands one dot into the following cycle. So the first dot
        // of this tick still belongs to the previous cycle's NMI sample
        // (ppu_vbl_nmi 05-08 measure this to the dot).
        self.ppu_step();
        self.sample_nmi_input_for_previous_cycle();
        self.ppu_step();
        self.ppu_step();
        self.apu.step();
        self.sample_irq_input();

        // Reload DMA: the output unit just emptied the sample buffer. The
        // DMA unit schedules its halt attempt for the next put cycle.
        if self.dmc_dma.is_none() && self.apu.dmc_fetch_address().is_some() {
            let attempt = Self::next_cycle_with_parity(self.total_cycles + 1, false);
            self.dmc_dma = Some(DmcDma {
                attempt,
                halted_at: None,
            });
        }

        if self.audio_capture {
            const CYCLES_PER_SAMPLE: f64 = 1_789_773.0 / 44_100.0;
            self.audio_acc.0 += self.apu.get_output();
            self.audio_acc.1 += 1;
            self.audio_sample_counter += 1.0;
            if self.audio_sample_counter >= CYCLES_PER_SAMPLE {
                self.audio_sample_counter -= CYCLES_PER_SAMPLE;
                let mean = self.audio_acc.0 / self.audio_acc.1 as f32;
                self.audio_acc = (0.0, 0);
                let sample = self.filter_audio(mean);
                self.audio_out.push(sample);
            }
        }
    }

    /// NES output stage: 90 Hz and 440 Hz first-order high-pass filters
    /// (the 14 kHz low-pass is above what 44.1 kHz needs). Coefficients are
    /// exp(-2*pi*f/44100). Scaled by 2 so full-scale mixer output spans
    /// roughly -1.0 to 1.0.
    fn filter_audio(&mut self, input: f32) -> f32 {
        const COEFFS: [f32; 2] = [0.987_25, 0.939_28];
        let mut x = input;
        for (hp, coeff) in self.audio_hp.iter_mut().zip(COEFFS) {
            let (prev_in, prev_out) = *hp;
            let y = coeff * (prev_out + x - prev_in);
            *hp = (x, y);
            x = y;
        }
        x * 2.0
    }

    fn read_byte(&mut self, addr: u16) -> u8 {
        self.tick();
        // A scheduled DMC DMA halts the CPU on this read cycle; the read
        // is repeated after the DMA (see `dmc_dma_stall`).
        if self.dmc_halt_due() {
            self.dmc_dma_stall(Some(addr));
        }
        // NMI is sampled one dot into the next tick (see `tick`), so an
        // access that drops the line here withdraws the edge in time.
        self.bus_read(addr)
    }

    /// DMA units can only read on "get" cycles and write on "put" cycles,
    /// which alternate with the APU clock. The CPU and APU power up in a
    /// random phase, so which CPU cycle parity is a get is arbitrary; this
    /// is the convention OAM DMA has always used (reads on odd
    /// `total_cycles`).
    fn is_get_cycle(tick: u64) -> bool {
        tick % 2 == 1
    }

    /// First tick at or after `from` that is a get (`get == true`) or a put
    /// cycle.
    fn next_cycle_with_parity(from: u64, get: bool) -> u64 {
        if Self::is_get_cycle(from) == get {
            from
        } else {
            from + 1
        }
    }

    /// True when a scheduled DMC DMA may halt the CPU on the cycle that
    /// just ticked. A request whose sample has since been stopped (a
    /// `$4015` write cleared the channel) is dropped.
    fn dmc_halt_due(&mut self) -> bool {
        match self.dmc_dma {
            Some(req) if req.attempt <= self.total_cycles => {
                if self.apu.dmc_fetch_address().is_some() {
                    true
                } else {
                    self.dmc_dma = None;
                    false
                }
            }
            _ => false,
        }
    }

    /// DMC DMA landing on a CPU read of `addr`; the tick that just ran is
    /// the halt cycle. The DMA unit then spends a dummy cycle, an alignment
    /// cycle if the next cycle is not a get, and reads the sample byte on
    /// the get. The CPU resumes by performing the interrupted read again
    /// (the caller's `bus_read`). While halted, the 2A03 keeps driving the
    /// interrupted read on every no-operation cycle, which is what makes a
    /// `$2007` or `$4016` read during DMC DMA lose data
    /// (docs/debugging/DMC_DMA.md).
    fn dmc_dma_stall(&mut self, addr: Option<u16>) {
        let halt_tick = self.instr_cycles;
        let start = self.total_cycles;
        // Halt cycle.
        self.repeat_halted_read(addr, true);
        // Dummy cycle.
        self.tick();
        self.repeat_halted_read(addr, false);
        // Alignment cycle.
        if !Self::is_get_cycle(self.total_cycles + 1) {
            self.tick();
            self.repeat_halted_read(addr, false);
        }
        // The DMA get.
        self.tick();
        self.dmc_fetch();
        // The CPU's own read, repeated.
        self.tick();
        let stall = (self.total_cycles - start) as u16;
        self.dma_cycles += stall;
        self.dmc_stall = (halt_tick, stall);
    }

    /// The read a halted CPU keeps performing. Each no-operation cycle is a
    /// separate access for PPU and APU registers (`$2007` advances its
    /// address every time). The joypad output enables stay asserted across
    /// adjacent reads of the same register, so the controllers see one
    /// clock for the whole halt/dummy/alignment set: only the halt cycle's
    /// read is performed for `$4016`/`$4017`.
    ///
    /// `None` stands for a padded cycle (see `cpu_step`): the dummy or
    /// internal read the 6502 performs there has no modelled address, so
    /// nothing is repeated.
    fn repeat_halted_read(&mut self, addr: Option<u16>, halt_cycle: bool) {
        let Some(addr) = addr else { return };
        if matches!(addr, 0x4016 | 0x4017) && !halt_cycle {
            return;
        }
        let _ = self.bus_read(addr);
    }

    /// The DMC get: read the sample byte through the bus and hand it to the
    /// APU. Clears the request whether or not the APU still wants a byte.
    fn dmc_fetch(&mut self) {
        if let Some(addr) = self.apu.dmc_fetch_address() {
            let byte = self.bus_read(addr);
            self.apu.dmc_supply_sample(byte);
        }
        self.dmc_dma = None;
    }

    /// Inside OAM DMA the CPU is already halted, so a scheduled DMC DMA
    /// halts on any cycle. Called after every OAM DMA tick.
    fn dmc_halt_during_oam(&mut self) {
        if let Some(req) = &mut self.dmc_dma {
            if req.halted_at.is_none() && req.attempt <= self.total_cycles {
                req.halted_at = Some(self.total_cycles);
            }
        }
    }

    /// True when a DMC DMA halted during OAM DMA is owed its get on the
    /// next cycle: the first get cycle at least two cycles after the halt
    /// (halt, dummy, then the get; an alignment cycle is implied when the
    /// cycle after the dummy is a put).
    fn dmc_get_due_next_cycle(&self) -> bool {
        let next = self.total_cycles + 1;
        Self::is_get_cycle(next)
            && matches!(self.dmc_dma, Some(DmcDma { halted_at: Some(h), .. }) if next >= h + 2)
    }

    /// OAM DMA (nesdev "DMA"). The DMA unit halts the CPU on the cycle
    /// after the `$4014` write, spends an alignment cycle if needed so its
    /// reads land on get cycles, then performs 256 get/put pairs: 513 or
    /// 514 cycles. Every cycle ticks, so the PPU and APU keep running.
    ///
    /// A DMC DMA scheduled while OAM DMA holds the CPU halts on any cycle,
    /// its get takes precedence over the OAM get, and OAM DMA then needs
    /// one alignment cycle to get back onto a get: 2 extra cycles in the
    /// common case, 1 when the DMC halt lands on the second-to-last put and
    /// 3 when it lands on the last put.
    fn oam_dma(&mut self, page: u16) {
        let start = self.total_cycles;
        // Halt cycle.
        self.tick();
        self.dmc_halt_during_oam();
        let mut index = 0u16;
        while index < 256 {
            if !Self::is_get_cycle(self.total_cycles + 1) {
                // Alignment cycle.
                self.tick();
                self.dmc_halt_during_oam();
                continue;
            }
            if self.dmc_get_due_next_cycle() {
                self.tick();
                self.dmc_fetch();
                continue;
            }
            self.tick();
            self.dmc_halt_during_oam();
            let data = self.bus_read(page | index);
            self.tick();
            self.dmc_halt_during_oam();
            self.bus_write(0x2004, data);
            index += 1;
        }
        // A DMC DMA that halted near the end of the transfer still owes
        // its get.
        if let Some(DmcDma {
            halted_at: Some(_), ..
        }) = self.dmc_dma
        {
            while !self.dmc_get_due_next_cycle() {
                self.tick();
            }
            self.tick();
            self.dmc_fetch();
        }
        self.dma_cycles += (self.total_cycles - start) as u16;
    }

    fn bus_read(&mut self, addr: u16) -> u8 {
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
        self.bus_write(addr, value);
    }

    fn bus_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.cpu_ram[(addr & 0x07FF) as usize] = value,
            0x2000..=0x3FFF => {
                let (ppu, mapper) = self.ppu_and_mapper();
                ppu.write_register(0x2000 | (addr & 0x0007), value, mapper)
            }
            0x4000..=0x4013 => self.apu.write_register(addr, value),
            0x4014 => self.oam_dma((value as u16) << 8),
            0x4015 => {
                self.apu.write_register(addr, value);
                // Load DMA: enabling the DMC with an empty sample buffer
                // schedules a halt on the get cycle 3 or 4 cycles after
                // this write. Clearing the channel cancels any request.
                match self.apu.dmc_fetch_address() {
                    Some(_) if self.dmc_dma.is_none() => {
                        let attempt = Self::next_cycle_with_parity(self.total_cycles + 3, true);
                        self.dmc_dma = Some(DmcDma {
                            attempt,
                            halted_at: None,
                        });
                    }
                    Some(_) => {}
                    None => self.dmc_dma = None,
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

    /// The dummy read an indexed store performs on the cycle before its
    /// write, at the address before the page-crossing fix-up (the high
    /// byte of `base` with the low byte of `addr`). It is a real bus access:
    /// `STA $2007,X` reads `$2007` and then writes it
    /// (dmc_dma_during_read4/read_write_2007).
    fn dummy_read_before_indexed_store(&mut self, base: u16, addr: u16) {
        let _ = self.read_byte((base & 0xFF00) | (addr & 0x00FF));
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
        self.dma_cycles = 0;
        self.dmc_stall = (0, 0);
        self.padding_is_write = false;
        self.irq_hist = 0;
        self.poll_tick = 0;
        self.no_poll = false;
        self.i_flag_for_poll = None;
        // An edge latched on the previous instruction's last tick (or left
        // over from a non-polling sequence) counts as pending from tick 0.
        self.nmi_seen_tick = 0;

        // The interrupt poll consumes the snapshot the previous instruction
        // took at its sample tick (see `tick` and `sample_tick`), never the
        // live inputs.
        let declared = match self.poll_interrupts() {
            Some(cycles) => cycles as u16,
            None => self.execute_opcode() as u16,
        };

        // The DMC memory reader's fetch is a real DMA: it halts the CPU on
        // a read cycle inside the instruction (see `read_byte` and
        // `dmc_dma_stall`), so there is nothing to do for it here.

        // DMA cycles inserted into the instruction (OAM DMA in the `$4014`
        // write, DMC DMA on any read) stretch it by exactly their length.
        debug_assert!(
            self.instr_cycles <= declared + self.dma_cycles,
            "instruction performed {} bus accesses but declares {} cycles",
            self.instr_cycles,
            declared + self.dma_cycles
        );
        // Padded cycles are reads on hardware (except the RMW dummy write),
        // so a scheduled DMC DMA halts on them like on any other read; the
        // stall lengthens the instruction by its own cycles.
        while self.instr_cycles < declared + self.dma_cycles {
            self.tick();
            if !self.padding_is_write && self.dmc_halt_due() {
                self.dmc_dma_stall(None);
            }
        }

        self.take_interrupt_snapshot(declared);
        self.instr_cycles
    }

    /// The tick of an instruction whose captured inputs the boundary poll
    /// uses: the penultimate cycle for everything except a taken branch
    /// that does not cross a page, which only polls before its second
    /// cycle (so an interrupt arriving during its last two cycles waits
    /// one more instruction).
    fn sample_tick(&self, declared: u16) -> u16 {
        if self.poll_tick != 0 {
            self.poll_tick
        } else {
            declared.saturating_sub(1).max(1)
        }
    }

    /// Capture the interrupt inputs at this tick. Called from `tick` after
    /// the PPU and APU have advanced and before the tick's bus access.
    /// NMI is sampled at the end of the CPU cycle, after the bus access, so
    /// an access that drops the PPU's NMI line in the same cycle the line
    /// rose (a `$2002` read or an NMI-disable write) withdraws the edge
    /// before the CPU sees it. Called after every bus access and after
    /// every padding tick.
    /// See `tick`: the NMI input is sampled one dot into the following
    /// cycle and attributed to the cycle before it, so the PPU has had that
    /// dot to withdraw an edge that a `$2002` read or `$2000` write made
    /// moot. At the start of an instruction that is tick 0, which the
    /// snapshot treats as "already pending when the instruction began".
    fn sample_nmi_input_for_previous_cycle(&mut self) {
        let tick = self.instr_cycles.saturating_sub(1);
        self.latch_nmi_edge(tick);
    }

    fn latch_nmi_edge(&mut self, tick: u16) {
        if self.ppu.nmi_interrupt {
            self.ppu.nmi_interrupt = false;
            // A second edge before the first is serviced is dropped, as on
            // hardware; only the first one's tick is remembered.
            if !self.nmi_pending {
                self.nmi_pending = true;
                self.nmi_seen_tick = tick;
            }
        }
    }

    fn sample_irq_input(&mut self) {
        if self.instr_cycles <= 16 && self.irq_line() {
            self.irq_hist |= 1 << (self.instr_cycles - 1);
        }
    }

    /// Level of the IRQ line: the OR of every source.
    fn irq_line(&self) -> bool {
        self.apu.irq_pending()
            || self.mapper_irq
            || self
                .cartridge
                .as_ref()
                .is_some_and(|cart| cart.mapper.irq_pending())
    }

    /// Decide, at the end of an instruction, which interrupt (if any) the
    /// next `cpu_step` services, from the inputs captured at the sample
    /// tick. BRK and the interrupt sequences never poll.
    fn take_interrupt_snapshot(&mut self, declared: u16) {
        if self.no_poll {
            self.sampled_nmi = false;
            self.sampled_irq = false;
            return;
        }
        let mut tick = self.sample_tick(declared);
        // A DMC DMA that halted the CPU at or before the sample tick pushed
        // the real penultimate cycle later by the length of the stall (the
        // halted CPU neither polls nor progresses; the interrupted cycle is
        // re-run after the DMA).
        let (halt_tick, stall) = self.dmc_stall;
        if halt_tick != 0 && halt_tick <= tick {
            tick += stall;
        }
        self.sampled_nmi = self.nmi_pending && self.nmi_seen_tick <= tick;
        let irq_level = tick <= 16 && self.irq_hist & (1 << (tick - 1)) != 0;
        let i_set = self.i_flag_for_poll.unwrap_or(self.cpu_status & 0x04 != 0);
        self.sampled_irq = !self.sampled_nmi && irq_level && !i_set;
    }

    fn execute_opcode(&mut self) -> u8 {
        let opcode = self.read_byte(self.cpu_pc);
        let old_pc = self.cpu_pc;
        self.cpu_pc = self.cpu_pc.wrapping_add(1);
        self.padding_is_write = is_read_modify_write(opcode);

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
                // SEI. The I flag change is polled one instruction late.
                self.i_flag_for_poll = Some(self.cpu_status & 0x04 != 0);
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
                //
                // Like the interrupt sequences, BRK does not poll at its
                // end, and an NMI edge seen by cycle 5 hijacks its vector.
                self.no_poll = true;
                self.read_byte(self.cpu_pc); // padding byte, discarded
                self.push_word(self.cpu_pc.wrapping_add(1));
                self.push(self.cpu_status | 0x30);
                self.cpu_status |= 0x04;
                let vector = self.brk_or_irq_vector();
                self.cpu_pc = self.read_word(vector);
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
                // PLP. Like CLI/SEI, the new I flag is polled one
                // instruction late (RTI is not delayed).
                self.i_flag_for_poll = Some(self.cpu_status & 0x04 != 0);
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
                self.dummy_read_before_indexed_store((hi << 8) | lo, addr);
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
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_y as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.dummy_read_before_indexed_store(base, addr);
                self.write_byte(addr, self.cpu_a);
                5
            }
            0x9D => {
                // STA absolute,X
                let base = self.read_word(self.cpu_pc);
                let addr = base.wrapping_add(self.cpu_x as u16);
                self.cpu_pc = self.cpu_pc.wrapping_add(2);
                self.dummy_read_before_indexed_store(base, addr);
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
                // CLI - Clear interrupt disable. The I flag change is polled
                // one instruction late: the next instruction always runs
                // before a pending IRQ is taken.
                self.i_flag_for_poll = Some(self.cpu_status & 0x04 != 0);
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
    ///
    /// A taken branch that does not cross a page polls interrupts only
    /// before its second cycle, so its sample tick is 1 rather than the
    /// penultimate cycle (cpu_interrupts_v2 test 5).
    fn branch_taken(&mut self, offset: i8) -> u8 {
        let next = self.cpu_pc;
        self.cpu_pc = next.wrapping_add(offset as u16);
        if Self::page_crossed(next, self.cpu_pc) {
            4
        } else {
            self.poll_tick = 1;
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

    /// Service the interrupt the previous instruction sampled, if any.
    /// Returns the cycles consumed (7) when an interrupt sequence ran.
    ///
    /// This consumes the snapshot taken by `take_interrupt_snapshot` at the
    /// previous instruction's sample tick, never the live inputs: an input
    /// asserted during an instruction's last cycle is not seen here.
    ///
    /// NMI is edge-triggered: the PPU raises `nmi_interrupt` exactly once per
    /// rising edge of its NMI output (vblank start with NMI enabled, or NMI
    /// enabled during vblank); `tick` latches it into `nmi_pending`, which
    /// is cleared here, so each edge is serviced once no matter how long the
    /// output stays high.
    ///
    /// IRQ is level-triggered: the line is the OR of every IRQ source and is
    /// serviced whenever it is high and the I flag is clear. The sources hold
    /// their flags until acknowledged, so a handler that never clears its
    /// source is re-entered as soon as it executes RTI, as on hardware.
    ///
    /// NMI has priority over IRQ.
    fn poll_interrupts(&mut self) -> Option<u8> {
        if self.sampled_nmi {
            self.sampled_nmi = false;
            self.sampled_irq = false;
            self.nmi_pending = false;
            self.nmi();
            return Some(7);
        }
        if self.sampled_irq {
            self.sampled_irq = false;
            self.irq();
            return Some(7);
        }
        None
    }

    fn nmi(&mut self) {
        self.no_poll = true;
        self.interrupt_dummy_reads();
        self.push_word(self.cpu_pc);
        // Hardware interrupts push P with B (bit 4) clear and bit 5 set.
        self.push((self.cpu_status & !0x10) | 0x20);
        self.cpu_status |= 0x04; // Set interrupt disable
        self.cpu_pc = self.read_word(0xFFFA);
    }

    fn irq(&mut self) {
        self.no_poll = true;
        self.interrupt_dummy_reads();
        self.push_word(self.cpu_pc);
        // Same vector as BRK, but B is clear so the handler can distinguish.
        self.push((self.cpu_status & !0x10) | 0x20);
        self.cpu_status |= 0x04; // Set interrupt disable
        let vector = self.brk_or_irq_vector();
        self.cpu_pc = self.read_word(vector);
    }

    /// Cycles 1 and 2 of the interrupt sequence: the opcode fetch that the
    /// interrupt replaced and the following dummy read, both discarded.
    fn interrupt_dummy_reads(&mut self) {
        self.read_byte(self.cpu_pc);
        self.read_byte(self.cpu_pc);
    }

    /// Interrupt hijacking: BRK and the IRQ sequence pick their vector
    /// during the P push (cycle 5). An NMI edge seen by then diverts them
    /// to the NMI vector, and the NMI is thereby serviced.
    fn brk_or_irq_vector(&mut self) -> u16 {
        if self.nmi_pending && self.nmi_seen_tick <= 4 {
            self.nmi_pending = false;
            0xFFFA
        } else {
            0xFFFE
        }
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

    /// Perform a CPU bus write with full side effects, ticking the PPU and
    /// APU as a real store would. For tests that need to program PPU or APU
    /// registers directly; game code should never need it.
    pub fn debug_write(&mut self, addr: u16, value: u8) {
        self.write_byte(addr, value);
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

    // ------------------------------------------------------------------
    // Battery-backed PRG RAM persistence (docs/debugging/BATTERY_SAVES.md).
    // ------------------------------------------------------------------

    /// PRG RAM of the loaded cartridge if, and only if, the iNES header
    /// flags it as battery backed and the mapper exposes PRG RAM.
    fn battery_ram(&self) -> Option<&[u8]> {
        let cart = self.cartridge.as_ref()?;
        if !cart.battery_backed {
            return None;
        }
        cart.mapper.prg_ram()
    }

    fn hash_ram(ram: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        ram.hash(&mut hasher);
        hasher.finish()
    }

    /// Restore battery-backed PRG RAM from `path`.
    ///
    /// Returns `Ok(false)` without touching RAM when the cartridge is not
    /// battery backed, when the file does not exist, or when its size does
    /// not match the board's PRG RAM (logged as a warning). Other I/O
    /// errors are returned. `Ok(true)` means RAM was replaced by the file.
    pub fn load_battery(&mut self, path: &Path) -> io::Result<bool> {
        let expected = match self.battery_ram() {
            Some(ram) => ram.len(),
            None => return Ok(false),
        };
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if data.len() != expected {
            log::warn!(
                "Ignoring battery save {}: {} bytes, expected {}",
                path.display(),
                data.len(),
                expected
            );
            return Ok(false);
        }
        let ram = self
            .cartridge
            .as_mut()
            .and_then(|cart| cart.mapper.prg_ram_mut())
            .expect("battery_ram() checked the cartridge and mapper");
        ram.copy_from_slice(&data);
        self.battery_saved_hash.set(Some(Self::hash_ram(ram)));
        log::info!(
            "Loaded battery save {} ({} bytes)",
            path.display(),
            expected
        );
        Ok(true)
    }

    /// Write battery-backed PRG RAM to `path` if it changed since the last
    /// load or save.
    ///
    /// Returns `Ok(false)` when the cartridge is not battery backed or when
    /// PRG RAM is identical to what was last loaded or written, so a caller
    /// may invoke this every few seconds; nothing is written in that case.
    /// `Ok(true)` means the file was (re)written.
    pub fn save_battery(&self, path: &Path) -> io::Result<bool> {
        let ram = match self.battery_ram() {
            Some(ram) => ram,
            None => return Ok(false),
        };
        let hash = Self::hash_ram(ram);
        if self.battery_saved_hash.get() == Some(hash) {
            return Ok(false);
        }
        std::fs::write(path, ram)?;
        self.battery_saved_hash.set(Some(hash));
        log::info!(
            "Wrote battery save {} ({} bytes)",
            path.display(),
            ram.len()
        );
        Ok(true)
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
    /// to completion inside the instruction, and NMI/IRQ are sampled on the
    /// instruction's penultimate cycle and serviced at the boundary, exactly
    /// as in `run_frame`.
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
