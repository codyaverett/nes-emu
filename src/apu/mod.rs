use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct ApuStatus: u8 {
        const PULSE1 = 0x01;
        const PULSE2 = 0x02;
        const TRIANGLE = 0x04;
        const NOISE = 0x08;
        const DMC = 0x10;
        const FRAME_INTERRUPT = 0x40;
        const DMC_INTERRUPT = 0x80;
    }
}

/// Length counter shared by the pulse, triangle and noise channels.
///
/// Models the hardware ordering that blargg's length-counter tests observe:
///
/// * A channel disabled through `$4015` has its counter forced to 0 and
///   ignores reloads until it is enabled again (apu_test 1 #7).
/// * A reload written to `$4003`/`$4007`/`$400B`/`$400F` and a halt-flag
///   change land one CPU cycle after the write, *after* that cycle's frame
///   clock. If the counter was clocked in between (it changed and was not
///   already 0) the reload is dropped, per blargg's `len_reload_timing`
///   readme ("reload during length clock when ctr > 0 should be ignored").
///   The halt flag likewise only guards clocks after the cycle it was
///   written in.
#[derive(Debug, Default, Clone, Copy)]
struct LengthCounter {
    enabled: bool,
    counter: u8,
    halt: bool,
    pending_halt: Option<bool>,
    pending_reload: Option<u8>,
    /// `counter` at the time of the pending reload's write.
    counter_at_write: u8,
}

impl LengthCounter {
    /// `$4015` enable bit: disabling clears the counter immediately.
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.counter = 0;
            self.pending_reload = None;
        }
    }

    /// Write of the length index (register bits 7-3); ignored while disabled.
    fn load(&mut self, index: u8) {
        if self.enabled {
            self.pending_reload = Some(LENGTH_TABLE[index as usize]);
            self.counter_at_write = self.counter;
        }
    }

    fn set_halt(&mut self, halt: bool) {
        self.pending_halt = Some(halt);
    }

    /// Half-frame clock.
    fn clock(&mut self) {
        if self.counter > 0 && !self.halt {
            self.counter -= 1;
        }
    }

    /// Land the previous cycle's register writes (called after the frame
    /// clock of the cycle following the write).
    fn apply_pending_writes(&mut self) {
        if let Some(value) = self.pending_reload.take() {
            if self.counter == self.counter_at_write {
                self.counter = value;
            }
        }
        if let Some(halt) = self.pending_halt.take() {
            self.halt = halt;
        }
    }

    fn active(&self) -> bool {
        self.counter > 0
    }

    /// Soft reset: `$4015` is cleared, so the channel is disabled and its
    /// counter zeroed. The halt flag is register state and survives.
    fn reset(&mut self) {
        self.set_enabled(false);
        self.pending_halt = None;
    }
}

pub struct Pulse {
    duty: u8,
    volume: u8,
    constant_volume: bool,
    envelope_loop: bool,
    envelope_period: u8,
    envelope_counter: u8,
    envelope_divider: u8,
    envelope_start: bool,
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    /// Sweep divider, counted down on each half-frame clock.
    sweep_divider: u8,
    /// Set by a `$4001`/`$4005` write; the divider reloads on the next
    /// half-frame clock.
    sweep_reload: bool,
    /// Pulse 1's sweep adder uses one's complement, so its negated target
    /// is one lower than pulse 2's.
    sweep_ones_complement: bool,
    timer_period: u16,
    timer_counter: u16,
    length: LengthCounter,
    sequence_pos: u8,
}

impl Pulse {
    fn new(sweep_ones_complement: bool) -> Self {
        Pulse {
            duty: 0,
            volume: 0,
            constant_volume: false,
            envelope_loop: false,
            envelope_period: 0,
            envelope_counter: 0,
            envelope_divider: 0,
            envelope_start: false,
            sweep_enabled: false,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_divider: 0,
            sweep_reload: false,
            sweep_ones_complement,
            timer_period: 0,
            timer_counter: 0,
            length: LengthCounter::default(),
            sequence_pos: 0,
        }
    }

    fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer_period;
            self.sequence_pos = (self.sequence_pos + 1) % 8;
        } else {
            self.timer_counter -= 1;
        }
    }

    /// The period the sweep unit would set: the current period plus or
    /// minus `period >> shift`. Pulse 1 subtracts one extra when negating
    /// (one's complement adder). The result is not clamped to 11 bits; a
    /// target above `0x7FF` mutes the channel instead.
    fn sweep_target(&self) -> u16 {
        let change = self.timer_period >> self.sweep_shift;
        if self.sweep_negate {
            let extra = u16::from(self.sweep_ones_complement);
            self.timer_period
                .saturating_sub(change)
                .saturating_sub(extra)
        } else {
            self.timer_period + change
        }
    }

    /// Sweep muting applies continuously, whatever the enable flag and
    /// shift count: a current period below 8 or a target above `0x7FF`
    /// silences the channel.
    fn sweep_muted(&self) -> bool {
        self.timer_period < 8 || self.sweep_target() > 0x7FF
    }

    /// Half-frame sweep clock. The period only changes when the divider is
    /// already 0 and the unit is enabled with a non-zero shift and not
    /// muting; the divider then reloads when it was 0 or a reload is
    /// pending, and counts down otherwise.
    fn clock_sweep(&mut self) {
        if self.sweep_divider == 0
            && self.sweep_enabled
            && self.sweep_shift != 0
            && !self.sweep_muted()
        {
            self.timer_period = self.sweep_target();
        }
        if self.sweep_divider == 0 || self.sweep_reload {
            self.sweep_divider = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_divider -= 1;
        }
    }

    fn _get_output(&self) -> u8 {
        if !self.length.active() || self.sweep_muted() {
            return 0;
        }

        let duty_table = [
            [0, 1, 0, 0, 0, 0, 0, 0],
            [0, 1, 1, 0, 0, 0, 0, 0],
            [0, 1, 1, 1, 1, 0, 0, 0],
            [1, 0, 0, 1, 1, 1, 1, 1],
        ];

        let sequence_output = duty_table[self.duty as usize][self.sequence_pos as usize];

        if sequence_output == 0 {
            0
        } else if self.constant_volume {
            self.volume
        } else {
            self.envelope_counter
        }
    }

    fn clock_envelope(&mut self) {
        if self.envelope_start {
            self.envelope_start = false;
            self.envelope_counter = 15;
            self.envelope_divider = self.envelope_period;
        } else {
            if self.envelope_divider > 0 {
                self.envelope_divider -= 1;
            } else {
                self.envelope_divider = self.envelope_period;
                if self.envelope_counter > 0 {
                    self.envelope_counter -= 1;
                } else if self.envelope_loop {
                    self.envelope_counter = 15;
                }
            }
        }
    }
}

pub struct Triangle {
    /// `$4008` bit 7: linear counter control flag, which is also the length
    /// counter halt flag. Survives a soft reset (apu_reset len_ctrs_enabled
    /// #3, "triangle unaffected").
    control: bool,
    linear_counter: u8,
    linear_counter_period: u8,
    linear_counter_reload: bool,
    timer_period: u16,
    timer_counter: u16,
    length: LengthCounter,
    sequence_pos: u8,
}

impl Triangle {
    fn new() -> Self {
        Triangle {
            control: false,
            linear_counter: 0,
            linear_counter_period: 0,
            linear_counter_reload: false,
            timer_period: 0,
            timer_counter: 0,
            length: LengthCounter::default(),
            sequence_pos: 0,
        }
    }

    fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer_period;
            if self.linear_counter > 0 && self.length.active() {
                self.sequence_pos = (self.sequence_pos + 1) % 32;
            }
        } else {
            self.timer_counter -= 1;
        }
    }

    /// Quarter-frame clock of the linear counter. The reload flag is only
    /// cleared when the control flag is clear, so a note with control set
    /// holds its linear counter at the period.
    fn clock_linear_counter(&mut self) {
        if self.linear_counter_reload {
            self.linear_counter = self.linear_counter_period;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control {
            self.linear_counter_reload = false;
        }
    }

    fn _get_output(&self) -> u8 {
        if !self.length.active() || self.linear_counter == 0 {
            return 0;
        }

        let triangle_table = [
            15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
            11, 12, 13, 14, 15,
        ];

        triangle_table[self.sequence_pos as usize]
    }
}

pub struct Noise {
    mode: bool,
    volume: u8,
    constant_volume: bool,
    envelope_loop: bool,
    envelope_period: u8,
    envelope_counter: u8,
    envelope_divider: u8,
    envelope_start: bool,
    timer_period: u16,
    timer_counter: u16,
    length: LengthCounter,
    shift_register: u16,
}

impl Noise {
    fn new() -> Self {
        Noise {
            mode: false,
            volume: 0,
            constant_volume: false,
            envelope_loop: false,
            envelope_period: 0,
            envelope_counter: 0,
            envelope_divider: 0,
            envelope_start: false,
            timer_period: 0,
            timer_counter: 0,
            length: LengthCounter::default(),
            shift_register: 1,
        }
    }

    fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer_period;
            let feedback_bit = if self.mode { 6 } else { 1 };
            let feedback = (self.shift_register & 1) ^ ((self.shift_register >> feedback_bit) & 1);
            self.shift_register >>= 1;
            self.shift_register |= feedback << 14;
        } else {
            self.timer_counter -= 1;
        }
    }

    fn _get_output(&self) -> u8 {
        if !self.length.active() || (self.shift_register & 1) == 1 {
            return 0;
        }

        if self.constant_volume {
            self.volume
        } else {
            self.envelope_counter
        }
    }

    fn clock_envelope(&mut self) {
        if self.envelope_start {
            self.envelope_start = false;
            self.envelope_counter = 15;
            self.envelope_divider = self.envelope_period;
        } else {
            if self.envelope_divider > 0 {
                self.envelope_divider -= 1;
            } else {
                self.envelope_divider = self.envelope_period;
                if self.envelope_counter > 0 {
                    self.envelope_counter -= 1;
                } else if self.envelope_loop {
                    self.envelope_counter = 15;
                }
            }
        }
    }
}

/// Frame sequencer schedule, in CPU cycles since the sequencer was last
/// restarted (the nesdev frame counter table, which is written in APU
/// cycles with half-cycle offsets, doubled). See
/// docs/debugging/APU_FRAME_COUNTER.md.
///
/// 4-step mode: quarter clocks at 7457, 14913, 22371 and 29829; half clocks
/// at 14913 and 29829; the frame IRQ flag is raised on 29828, 29829 and
/// 29830 (three consecutive cycles); the sequence wraps at 29830.
/// 5-step mode: quarter at 7457, 14913, 22371 and 37281; half at 14913 and
/// 37281; wraps at 37282; nothing happens at 29829.
const FRAME_QUARTER_1: u32 = 7457;
const FRAME_HALF_1: u32 = 14913;
const FRAME_QUARTER_3: u32 = 22371;
const FRAME_4STEP_IRQ_FIRST: u32 = 29828;
const FRAME_4STEP_HALF_2: u32 = 29829;
const FRAME_4STEP_PERIOD: u32 = 29830;
const FRAME_5STEP_HALF_2: u32 = 37281;
const FRAME_5STEP_PERIOD: u32 = 37282;

/// CPU cycles between a `$4017` write and the sequencer restart when the
/// write lands on an even APU-aligned cycle; odd cycles take one more. Power
/// and reset behave as a `$4017` write with this delay.
const FRAME_RESET_DELAY: u8 = 3;

/// NTSC DMC timer periods (CPU cycles per output bit), indexed by $4010 bits 0-3.
const DMC_RATE_TABLE: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

pub struct Dmc {
    enabled: bool,
    rate: u8,
    direct_load: u8,
    sample_address: u16,
    sample_length: u16,
    current_address: u16,
    bytes_remaining: u16,
    sample_buffer: Option<u8>,
    output_level: u8,
    shift_register: u8,
    bits_remaining: u8,
    silence_flag: bool,
    timer_counter: u16,
    irq_enabled: bool,
    loop_flag: bool,
    /// DMC interrupt flag: set when the last byte of a sample is consumed
    /// with IRQ enabled. Cleared by a $4015 write or by writing $4010 with
    /// bit 7 clear. Note that a $4015 read does NOT clear it (only the
    /// frame interrupt flag is cleared by reads).
    interrupt: bool,
}

impl Dmc {
    fn new() -> Self {
        Dmc {
            enabled: false,
            rate: 0,
            direct_load: 0,
            sample_address: 0xC000,
            sample_length: 1,
            current_address: 0xC000,
            bytes_remaining: 0,
            sample_buffer: None,
            output_level: 0,
            shift_register: 0,
            bits_remaining: 8,
            silence_flag: true,
            timer_counter: DMC_RATE_TABLE[0],
            irq_enabled: false,
            loop_flag: false,
            interrupt: false,
        }
    }

    fn _get_output(&self) -> u8 {
        self.output_level
    }

    /// Restart the sample from its programmed address and length.
    fn restart(&mut self) {
        self.current_address = self.sample_address;
        self.bytes_remaining = self.sample_length;
    }

    /// The memory reader wants a byte when the sample buffer is empty and
    /// there are bytes left in the current sample.
    fn wants_sample(&self) -> bool {
        self.sample_buffer.is_none() && self.bytes_remaining > 0
    }

    /// Supply the byte fetched from `current_address`. Advances the address
    /// (wrapping from $FFFF to $8000) and decrements the byte count. When the
    /// count hits zero the sample either loops or, with IRQ enabled, raises
    /// the DMC interrupt flag.
    fn supply_sample(&mut self, byte: u8) {
        self.sample_buffer = Some(byte);
        self.current_address = if self.current_address == 0xFFFF {
            0x8000
        } else {
            self.current_address + 1
        };
        self.bytes_remaining -= 1;
        if self.bytes_remaining == 0 {
            if self.loop_flag {
                self.restart();
            } else if self.irq_enabled {
                self.interrupt = true;
            }
        }
    }

    /// Clock the output unit timer once per CPU cycle.
    fn clock_timer(&mut self) {
        if self.timer_counter > 0 {
            self.timer_counter -= 1;
            return;
        }
        self.timer_counter = DMC_RATE_TABLE[self.rate as usize].saturating_sub(1);

        // Output unit: one bit per timer tick.
        if !self.silence_flag {
            if self.shift_register & 0x01 != 0 {
                if self.output_level <= 125 {
                    self.output_level += 2;
                }
            } else if self.output_level >= 2 {
                self.output_level -= 2;
            }
        }
        self.shift_register >>= 1;
        self.bits_remaining -= 1;

        if self.bits_remaining == 0 {
            // Start a new output cycle: pull the next byte from the buffer.
            self.bits_remaining = 8;
            match self.sample_buffer.take() {
                Some(byte) => {
                    self.silence_flag = false;
                    self.shift_register = byte;
                }
                None => self.silence_flag = true,
            }
        }
    }
}

pub struct Apu {
    pulse1: Pulse,
    pulse2: Pulse,
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    status: ApuStatus,
    /// Last value written to `$4017` (re-applied by a soft reset).
    frame_counter: u8,
    /// Sequencer mode in effect: true for the 5-step sequence. Latched from
    /// `frame_counter` bit 7 when the `$4017` write lands, not at the write.
    frame_5step: bool,
    /// CPU cycles since the sequencer was last restarted; indexes the
    /// `FRAME_*` schedule. Separate from `cycles` so a $4017 write can reset
    /// the sequencer without disturbing the channel timer parity.
    frame_cycle: u32,
    /// Countdown until a pending `$4017` write resets the sequencer. The
    /// reset lands 3 CPU cycles after a write on an even (APU) cycle and 4
    /// after a write on an odd cycle; 0 means nothing pending.
    frame_reset_delay: u8,
    /// Frame interrupt flag: raised on the last three cycles of the 4-step
    /// sequence unless inhibited. Held until a $4015 read or a $4017 write
    /// with bit 6 (inhibit) set.
    frame_interrupt: bool,
    frame_interrupt_inhibit: bool,
    cycles: u64,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        Apu {
            pulse1: Pulse::new(true),
            pulse2: Pulse::new(false),
            triangle: Triangle::new(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            status: ApuStatus::empty(),
            frame_counter: 0,
            frame_5step: false,
            frame_cycle: 0,
            // Power-up behaves as if $00 had been written to $4017: the
            // 4-step sequence starts (IRQ enabled) after the write delay.
            frame_reset_delay: FRAME_RESET_DELAY,
            frame_interrupt: false,
            frame_interrupt_inhibit: false,
            cycles: 0,
        }
    }

    /// Soft reset (the console's RESET button). Per apu_reset: `$4015` is
    /// cleared (all channels disabled, samples stopped, IRQ flags clear),
    /// the last `$4017` value is written again with the IRQ inhibit bit
    /// dropped, and the frame IRQ flag is clear. Everything else is
    /// register state that the reset line does not touch; in particular the
    /// triangle's control flag, linear counter and period survive
    /// (len_ctrs_enabled #3). The triangle sequencer phase restarts.
    pub fn reset(&mut self) {
        self.pulse1.length.reset();
        self.pulse2.length.reset();
        self.triangle.length.reset();
        self.triangle.sequence_pos = 0;
        self.noise.length.reset();
        self.dmc.enabled = false;
        self.dmc.bytes_remaining = 0;
        self.dmc.interrupt = false;
        self.status = ApuStatus::empty();
        self.frame_counter &= !0x40;
        // Park the sequencer until the re-write lands so a reset taken just
        // before the IRQ cycles cannot raise the flag in the meantime.
        self.frame_cycle = 0;
        self.frame_reset_delay = FRAME_RESET_DELAY;
        self.frame_interrupt = false;
        self.frame_interrupt_inhibit = false;
        self.cycles = 0;
    }

    pub fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x4015 => {
                let mut result = 0u8;
                if self.pulse1.length.active() {
                    result |= 0x01;
                }
                if self.pulse2.length.active() {
                    result |= 0x02;
                }
                if self.triangle.length.active() {
                    result |= 0x04;
                }
                if self.noise.length.active() {
                    result |= 0x08;
                }
                if self.dmc.bytes_remaining > 0 {
                    result |= 0x10;
                }
                if self.frame_interrupt {
                    result |= 0x40;
                }
                if self.dmc.interrupt {
                    result |= 0x80;
                }
                // Reading $4015 clears the frame interrupt flag only; the DMC
                // interrupt flag is cleared by writing $4015 (or $4010 with
                // bit 7 clear).
                self.frame_interrupt = false;
                result
            }
            _ => 0,
        }
    }

    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0x4000 => {
                self.pulse1.duty = (value >> 6) & 0x03;
                self.pulse1.envelope_loop = (value & 0x20) != 0;
                self.pulse1.length.set_halt((value & 0x20) != 0);
                self.pulse1.constant_volume = (value & 0x10) != 0;
                self.pulse1.volume = value & 0x0F;
                self.pulse1.envelope_period = value & 0x0F;
            }
            0x4001 => {
                self.pulse1.sweep_enabled = (value & 0x80) != 0;
                self.pulse1.sweep_period = (value >> 4) & 0x07;
                self.pulse1.sweep_negate = (value & 0x08) != 0;
                self.pulse1.sweep_shift = value & 0x07;
                self.pulse1.sweep_reload = true;
            }
            0x4002 => {
                self.pulse1.timer_period = (self.pulse1.timer_period & 0xFF00) | value as u16;
            }
            0x4003 => {
                self.pulse1.timer_period =
                    (self.pulse1.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.pulse1.length.load(value >> 3);
                self.pulse1.envelope_start = true;
            }

            0x4004 => {
                self.pulse2.duty = (value >> 6) & 0x03;
                self.pulse2.envelope_loop = (value & 0x20) != 0;
                self.pulse2.length.set_halt((value & 0x20) != 0);
                self.pulse2.constant_volume = (value & 0x10) != 0;
                self.pulse2.volume = value & 0x0F;
                self.pulse2.envelope_period = value & 0x0F;
            }
            0x4005 => {
                self.pulse2.sweep_enabled = (value & 0x80) != 0;
                self.pulse2.sweep_period = (value >> 4) & 0x07;
                self.pulse2.sweep_negate = (value & 0x08) != 0;
                self.pulse2.sweep_shift = value & 0x07;
                self.pulse2.sweep_reload = true;
            }
            0x4006 => {
                self.pulse2.timer_period = (self.pulse2.timer_period & 0xFF00) | value as u16;
            }
            0x4007 => {
                self.pulse2.timer_period =
                    (self.pulse2.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.pulse2.length.load(value >> 3);
                self.pulse2.envelope_start = true;
            }

            0x4008 => {
                self.triangle.control = (value & 0x80) != 0;
                self.triangle.length.set_halt((value & 0x80) != 0);
                self.triangle.linear_counter_period = value & 0x7F;
            }
            0x400A => {
                self.triangle.timer_period = (self.triangle.timer_period & 0xFF00) | value as u16;
            }
            0x400B => {
                self.triangle.timer_period =
                    (self.triangle.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.triangle.length.load(value >> 3);
                self.triangle.linear_counter_reload = true;
            }

            0x400C => {
                self.noise.envelope_loop = (value & 0x20) != 0;
                self.noise.length.set_halt((value & 0x20) != 0);
                self.noise.constant_volume = (value & 0x10) != 0;
                self.noise.volume = value & 0x0F;
                self.noise.envelope_period = value & 0x0F;
            }
            0x400E => {
                self.noise.mode = (value & 0x80) != 0;
                self.noise.timer_period = NOISE_PERIOD_TABLE[(value & 0x0F) as usize];
            }
            0x400F => {
                self.noise.length.load(value >> 3);
                self.noise.envelope_start = true;
            }

            0x4010 => {
                self.dmc.irq_enabled = (value & 0x80) != 0;
                self.dmc.loop_flag = (value & 0x40) != 0;
                self.dmc.rate = value & 0x0F;
                if !self.dmc.irq_enabled {
                    self.dmc.interrupt = false;
                }
            }
            0x4011 => {
                self.dmc.direct_load = value & 0x7F;
                self.dmc.output_level = value & 0x7F;
            }
            0x4012 => {
                self.dmc.sample_address = 0xC000 | ((value as u16) << 6);
            }
            0x4013 => {
                self.dmc.sample_length = ((value as u16) << 4) | 1;
            }

            0x4015 => {
                // Disabling a channel clears its length counter at once and
                // blocks reloads until it is enabled again.
                self.pulse1.length.set_enabled((value & 0x01) != 0);
                self.pulse2.length.set_enabled((value & 0x02) != 0);
                self.triangle.length.set_enabled((value & 0x04) != 0);
                self.noise.length.set_enabled((value & 0x08) != 0);
                self.dmc.enabled = (value & 0x10) != 0;

                // Bit 4: restart the sample if one is not already playing;
                // clearing it stops playback once the buffer drains.
                if self.dmc.enabled {
                    if self.dmc.bytes_remaining == 0 {
                        self.dmc.restart();
                    }
                } else {
                    self.dmc.bytes_remaining = 0;
                }

                // Any write to $4015 clears the DMC interrupt flag.
                self.dmc.interrupt = false;
            }

            0x4017 => {
                self.frame_counter = value;
                self.frame_interrupt_inhibit = (value & 0x40) != 0;
                if self.frame_interrupt_inhibit {
                    self.frame_interrupt = false;
                }

                // Writing $4017 resets the frame sequencer, but the reset
                // (and the mode change) lands 3 or 4 CPU cycles after the
                // write depending on the write cycle's parity. See
                // `apply_frame_reset`.
                self.frame_reset_delay = if self.cycles.is_multiple_of(2) {
                    FRAME_RESET_DELAY
                } else {
                    FRAME_RESET_DELAY + 1
                };
            }

            _ => {}
        }
    }

    pub fn step(&mut self) {
        if self.cycles.is_multiple_of(2) {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
            self.noise.clock_timer();
        }

        self.triangle.clock_timer();
        self.dmc.clock_timer();

        // A pending $4017 write landing this cycle restarts the sequencer
        // in place of advancing it.
        let landed = self.frame_reset_delay > 0 && {
            self.frame_reset_delay -= 1;
            self.frame_reset_delay == 0
        };
        if landed {
            self.apply_frame_reset();
        } else {
            self.frame_cycle += 1;
            self.clock_frame_counter();
        }

        // Length counter reloads and halt writes from the previous cycle
        // land after this cycle's frame clock (see `LengthCounter`).
        self.pulse1.length.apply_pending_writes();
        self.pulse2.length.apply_pending_writes();
        self.triangle.length.apply_pending_writes();
        self.noise.length.apply_pending_writes();

        self.cycles += 1;
    }

    /// Delayed effect of a `$4017` write: latch the mode, restart the
    /// sequencer and, in 5-step mode, clock the quarter- and half-frame
    /// units immediately.
    fn apply_frame_reset(&mut self) {
        self.frame_5step = (self.frame_counter & 0x80) != 0;
        self.frame_cycle = 0;
        if self.frame_5step {
            self.clock_quarter_frame();
            self.clock_half_frame();
        }
    }

    /// Level of the APU's contribution to the CPU IRQ line: the OR of the
    /// frame interrupt flag and the DMC interrupt flag.
    pub fn irq_pending(&self) -> bool {
        self.frame_interrupt || self.dmc.interrupt
    }

    /// If the DMC memory reader needs a byte, returns the address to fetch.
    /// The bus owner (System) reads it and hands it back via
    /// [`Apu::dmc_supply_sample`]. This request/supply handshake keeps the
    /// APU free of any reference to the CPU bus.
    pub fn dmc_fetch_address(&self) -> Option<u16> {
        if self.dmc.wants_sample() {
            Some(self.dmc.current_address)
        } else {
            None
        }
    }

    /// Deliver the byte requested by [`Apu::dmc_fetch_address`].
    pub fn dmc_supply_sample(&mut self, byte: u8) {
        self.dmc.supply_sample(byte);
    }

    /// Run the sequencer action scheduled for `frame_cycle`, if any, and
    /// wrap at the end of the sequence.
    fn clock_frame_counter(&mut self) {
        if self.frame_5step {
            match self.frame_cycle {
                FRAME_QUARTER_1 | FRAME_QUARTER_3 => self.clock_quarter_frame(),
                FRAME_HALF_1 | FRAME_5STEP_HALF_2 => {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                }
                FRAME_5STEP_PERIOD => self.frame_cycle = 0,
                _ => {}
            }
        } else {
            match self.frame_cycle {
                FRAME_QUARTER_1 | FRAME_QUARTER_3 => self.clock_quarter_frame(),
                FRAME_HALF_1 => {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                }
                FRAME_4STEP_IRQ_FIRST => self.raise_frame_interrupt(),
                FRAME_4STEP_HALF_2 => {
                    self.raise_frame_interrupt();
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                }
                FRAME_4STEP_PERIOD => {
                    self.raise_frame_interrupt();
                    self.frame_cycle = 0;
                }
                _ => {}
            }
        }
    }

    /// The 4-step sequence raises the frame interrupt flag on each of its
    /// last three cycles unless inhibited by $4017 bit 6.
    fn raise_frame_interrupt(&mut self) {
        if !self.frame_interrupt_inhibit {
            self.frame_interrupt = true;
        }
    }

    /// Quarter-frame clock: envelopes and the triangle linear counter.
    fn clock_quarter_frame(&mut self) {
        self.pulse1.clock_envelope();
        self.pulse2.clock_envelope();
        self.noise.clock_envelope();
        self.triangle.clock_linear_counter();
    }

    /// Half-frame clock: length counters and sweep units.
    fn clock_half_frame(&mut self) {
        self.pulse1.length.clock();
        self.pulse2.length.clock();
        self.triangle.length.clock();
        self.noise.length.clock();
        self.clock_sweeps();
    }

    fn clock_sweeps(&mut self) {
        self.pulse1.clock_sweep();
        self.pulse2.clock_sweep();
    }

    pub fn get_output(&self) -> f32 {
        let pulse1 = self.pulse1._get_output() as f32;
        let pulse2 = self.pulse2._get_output() as f32;
        let triangle = self.triangle._get_output() as f32;
        let noise = self.noise._get_output() as f32;
        let dmc = self.dmc._get_output() as f32;

        // Mix the channels using the NES non-linear mixing formula
        let pulse_out = if pulse1 + pulse2 > 0.0 {
            95.52 / (8128.0 / (pulse1 + pulse2) + 100.0)
        } else {
            0.0
        };

        let tnd_out = if triangle + noise + dmc > 0.0 {
            159.79 / (1.0 / (triangle / 8227.0 + noise / 12241.0 + dmc / 22638.0) + 100.0)
        } else {
            0.0
        };

        // Normalize output from 0.0-0.5 range to -1.0 to +1.0 range for SDL2 audio
        // First multiply by 2 to get 0.0-1.0, then transform to -1.0 to +1.0
        let output = (pulse_out + tnd_out) * 2.0;
        output * 2.0 - 1.0
    }
}

const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

const NOISE_PERIOD_TABLE: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn run(apu: &mut Apu, cycles: u32) {
        for _ in 0..cycles {
            apu.step();
        }
    }

    /// Write `$4017` and step through the 3-4 cycle delay before the
    /// sequencer reset lands, so the tests count from the reset itself.
    fn write_4017(apu: &mut Apu, value: u8) {
        apu.write_register(0x4017, value);
        while apu.frame_reset_delay > 0 {
            apu.step();
        }
    }

    /// Read `$4015` bit 6 (and clear it).
    fn frame_flag(apu: &mut Apu) -> bool {
        apu.read_register(0x4015) & 0x40 != 0
    }

    #[test]
    fn frame_irq_set_at_end_of_4_step_sequence() {
        let mut apu = Apu::new();
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_QUARTER_3);
        assert!(!apu.irq_pending(), "not before the fourth step");
        run(&mut apu, FRAME_4STEP_IRQ_FIRST - FRAME_QUARTER_3);
        assert!(apu.irq_pending(), "set on the fourth step");
        assert!(frame_flag(&mut apu));
        assert!(!apu.irq_pending(), "$4015 read clears the frame flag");
        run(&mut apu, 3);
        assert!(frame_flag(&mut apu), "re-set by the rest of the window");
        run(&mut apu, 1);
        assert!(!frame_flag(&mut apu), "stays clear once the window passed");
    }

    /// apu_test 6: the flag is raised on three consecutive cycles
    /// (29828-29830 after the restart, 29831-29833 after an even-cycle
    /// write) and each of them re-sets a flag cleared by a `$4015` read.
    #[test]
    fn frame_irq_flag_window_is_three_cycles() {
        let mut apu = Apu::new();
        run(&mut apu, 10); // even cycle: 3-cycle write delay
        apu.write_register(0x4017, 0x00);
        run(&mut apu, 29830);
        assert!(!frame_flag(&mut apu), "clear at write+29830");
        run(&mut apu, 1);
        assert!(frame_flag(&mut apu), "first set at write+29831");
        run(&mut apu, 1);
        assert!(frame_flag(&mut apu), "set again at write+29832");
        run(&mut apu, 1);
        assert!(frame_flag(&mut apu), "last set at write+29833");
        run(&mut apu, 1);
        assert!(!frame_flag(&mut apu), "not set at write+29834");
        run(&mut apu, 100);
        assert!(!frame_flag(&mut apu));
    }

    /// The first flag comes 29831 cycles after a `$4017` write (3-cycle
    /// delay plus 29828), 29832 after an odd-cycle write, and every 29830
    /// cycles after that.
    #[test]
    fn frame_irq_period_is_29830_after_a_29831_first_frame() {
        for odd in [false, true] {
            let mut apu = Apu::new();
            run(&mut apu, if odd { 11 } else { 10 });
            apu.write_register(0x4017, 0x00);
            let first = if odd { 29832 } else { 29831 };
            run(&mut apu, first - 1);
            assert!(!frame_flag(&mut apu), "odd={odd}: clear before first");
            run(&mut apu, 1);
            assert!(frame_flag(&mut apu), "odd={odd}: first flag");
            // Clear the two remaining cycles of the window.
            run(&mut apu, 2);
            frame_flag(&mut apu);
            for frame in 1..4 {
                run(&mut apu, FRAME_4STEP_PERIOD - 3);
                assert!(!frame_flag(&mut apu), "odd={odd}: frame {frame} early");
                run(&mut apu, 1);
                assert!(frame_flag(&mut apu), "odd={odd}: frame {frame} on time");
                run(&mut apu, 2);
                frame_flag(&mut apu);
            }
        }
    }

    #[test]
    fn frame_irq_held_until_acknowledged() {
        let mut apu = Apu::new();
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_4STEP_PERIOD);
        assert!(apu.irq_pending());
        run(&mut apu, FRAME_4STEP_PERIOD * 3);
        assert!(apu.irq_pending(), "flag is held, not pulsed");
    }

    #[test]
    fn frame_irq_inhibited_and_cleared_by_4017_bit6() {
        let mut apu = Apu::new();
        write_4017(&mut apu, 0x40);
        run(&mut apu, FRAME_4STEP_PERIOD * 2);
        assert!(!apu.irq_pending(), "inhibit bit blocks the flag");

        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_4STEP_PERIOD);
        assert!(apu.irq_pending());
        write_4017(&mut apu, 0x40);
        assert!(!apu.irq_pending(), "writing $4017 with inhibit clears it");
    }

    #[test]
    fn frame_irq_never_set_in_5_step_mode() {
        let mut apu = Apu::new();
        write_4017(&mut apu, 0x80);
        run(&mut apu, FRAME_5STEP_PERIOD * 3);
        assert!(!apu.irq_pending());
    }

    #[test]
    fn write_4017_resets_sequencer() {
        let mut apu = Apu::new();
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_QUARTER_3 + 100);
        // Reset: the flag is now a full sequence away again.
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_4STEP_IRQ_FIRST - 1);
        assert!(!apu.irq_pending());
        run(&mut apu, 1);
        assert!(apu.irq_pending());
    }

    #[test]
    fn write_4017_5_step_clocks_length_counters_immediately() {
        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x08); // length index 1 -> 254
        run(&mut apu, 1); // the reload lands on the next cycle
        assert_eq!(apu.pulse1.length.counter, 254);
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.length.counter, 253);
        write_4017(&mut apu, 0x00);
        assert_eq!(
            apu.pulse1.length.counter, 253,
            "4-step write does not clock"
        );
    }

    /// apu_test 5: half-frame clocks at 14913 and 29829 after the restart
    /// in 4-step mode; at 0 (immediate), 14913 and 37281 in 5-step mode.
    #[test]
    fn length_counter_clock_schedule() {
        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x28); // length 4
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_HALF_1 - 1);
        assert_eq!(apu.pulse1.length.counter, 4);
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 3);
        run(&mut apu, FRAME_4STEP_HALF_2 - FRAME_HALF_1);
        assert_eq!(apu.pulse1.length.counter, 2);
        run(
            &mut apu,
            FRAME_4STEP_PERIOD + FRAME_HALF_1 - FRAME_4STEP_HALF_2,
        );
        assert_eq!(apu.pulse1.length.counter, 1);

        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x28);
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.length.counter, 3, "immediate clock");
        run(&mut apu, FRAME_HALF_1);
        assert_eq!(apu.pulse1.length.counter, 2);
        run(&mut apu, FRAME_5STEP_HALF_2 - FRAME_HALF_1 - 1);
        assert_eq!(apu.pulse1.length.counter, 2);
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 1);
    }

    /// apu_test 1 #6-#8: disabled channels neither keep nor reload a
    /// length; the halt flag suspends clocking.
    #[test]
    fn length_counter_enable_and_halt() {
        let mut apu = Apu::new();
        apu.write_register(0x4003, 0x28);
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 0, "disabled: no reload");
        assert_eq!(apu.read_register(0x4015) & 0x0F, 0);

        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x28);
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 4);
        apu.write_register(0x4015, 0x00);
        assert_eq!(apu.pulse1.length.counter, 0, "disable clears at once");

        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4000, 0x30); // halt
        apu.write_register(0x4003, 0x28);
        run(&mut apu, 1);
        write_4017(&mut apu, 0x80);
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.length.counter, 4, "halted counter holds");
    }

    /// A reload that races a length clock on the following cycle loses when
    /// the counter was non-zero; the halt flag written in the same cycle as
    /// a clock does not stop that clock.
    #[test]
    fn length_reload_and_halt_land_after_the_clock() {
        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x28); // 4
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_HALF_1 - 1);
        apu.write_register(0x4003, 0x08); // 254, written the cycle before the clock
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 3, "clocked reload is ignored");

        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_HALF_1 - 1);
        apu.write_register(0x4003, 0x08);
        run(&mut apu, 1);
        assert_eq!(
            apu.pulse1.length.counter, 254,
            "reload of a zero counter works"
        );

        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x28);
        write_4017(&mut apu, 0x00);
        run(&mut apu, FRAME_HALF_1 - 1);
        apu.write_register(0x4000, 0x30); // halt, lands after the clock
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 3, "clock still happens");
        run(&mut apu, FRAME_4STEP_HALF_2 - FRAME_HALF_1);
        assert_eq!(apu.pulse1.length.counter, 3, "halted from then on");
    }

    /// apu_reset: power-up is as if $00 were written to $4017 (sequencer
    /// running, IRQ enabled), with $4015 reading 0.
    #[test]
    fn power_up_state() {
        let mut apu = Apu::new();
        assert_eq!(apu.read_register(0x4015), 0);
        run(
            &mut apu,
            FRAME_RESET_DELAY as u32 + FRAME_4STEP_IRQ_FIRST - 1,
        );
        assert!(!apu.irq_pending());
        run(&mut apu, 1);
        assert!(apu.irq_pending(), "frame IRQ runs from power-up");
    }

    /// apu_reset: reset clears $4015, re-writes the last $4017 value and
    /// keeps the triangle's control flag.
    #[test]
    fn reset_clears_4015_rewrites_4017_and_keeps_triangle_control() {
        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x0F);
        apu.write_register(0x4008, 0xFF); // triangle control/halt
        apu.write_register(0x4003, 0x28);
        apu.write_register(0x400B, 0x28);
        write_4017(&mut apu, 0x80);
        run(&mut apu, 100);
        assert_eq!(apu.read_register(0x4015) & 0x0F, 0x05);

        apu.reset();
        assert_eq!(apu.read_register(0x4015), 0, "$4015 cleared");
        assert!(!apu.irq_pending());
        apu.write_register(0x4003, 0x28);
        run(&mut apu, 1);
        assert_eq!(apu.pulse1.length.counter, 0, "channels disabled");
        assert!(apu.triangle.control, "triangle control survives");

        // The 5-step mode is re-applied after the reset delay, so no frame
        // IRQ appears.
        run(&mut apu, FRAME_5STEP_PERIOD * 2);
        assert!(!apu.irq_pending());
        assert!(apu.frame_5step);

        // A halted triangle keeps its length once re-enabled.
        apu.write_register(0x4015, 0x0F);
        apu.write_register(0x400B, 0x18); // 2
        apu.write_register(0x4003, 0x18);
        run(&mut apu, FRAME_5STEP_PERIOD * 2);
        assert_eq!(apu.read_register(0x4015) & 0x0F, 0x04);
    }

    /// Feed the DMC every byte it asks for while stepping.
    fn run_dmc(apu: &mut Apu, cycles: u32) {
        for _ in 0..cycles {
            if apu.dmc_fetch_address().is_some() {
                apu.dmc_supply_sample(0xAA);
            }
            apu.step();
        }
    }

    #[test]
    fn dmc_irq_set_when_sample_finishes_with_irq_enabled() {
        let mut apu = Apu::new();
        apu.write_register(0x4010, 0x8F); // IRQ on, no loop, fastest rate
        apu.write_register(0x4012, 0x00); // $C000
        apu.write_register(0x4013, 0x00); // length 1 byte
        apu.write_register(0x4015, 0x10); // start
        assert_eq!(apu.dmc_fetch_address(), Some(0xC000));
        assert!(!apu.irq_pending());

        // One byte at rate 54 cycles/bit finishes within a few hundred cycles.
        run_dmc(&mut apu, 54 * 8 * 3);
        assert!(apu.irq_pending(), "DMC IRQ raised when the sample ends");
        assert_ne!(apu.read_register(0x4015) & 0x80, 0);
        assert!(
            apu.irq_pending(),
            "$4015 read does not clear the DMC flag (hardware behaviour)"
        );

        apu.write_register(0x4015, 0x00);
        assert!(!apu.irq_pending(), "$4015 write clears the DMC flag");
    }

    #[test]
    fn dmc_irq_not_set_when_disabled_or_looping() {
        let mut apu = Apu::new();
        apu.write_register(0x4010, 0x0F); // IRQ off
        apu.write_register(0x4013, 0x00);
        apu.write_register(0x4015, 0x10);
        run_dmc(&mut apu, 54 * 8 * 3);
        assert!(!apu.irq_pending());

        let mut apu = Apu::new();
        apu.write_register(0x4010, 0xCF); // IRQ on but looping
        apu.write_register(0x4013, 0x00);
        apu.write_register(0x4015, 0x10);
        run_dmc(&mut apu, 54 * 8 * 6);
        assert!(!apu.irq_pending(), "looping samples never finish");
        assert!(apu.dmc.bytes_remaining > 0, "loop restarted the sample");
    }

    #[test]
    fn dmc_irq_cleared_by_4010_with_bit7_clear() {
        let mut apu = Apu::new();
        apu.write_register(0x4010, 0x8F);
        apu.write_register(0x4013, 0x00);
        apu.write_register(0x4015, 0x10);
        run_dmc(&mut apu, 54 * 8 * 3);
        assert!(apu.irq_pending());
        apu.write_register(0x4010, 0x0F);
        assert!(!apu.irq_pending());
    }

    // ---- Sweep units ----

    /// Enable pulse 1 with duty 3 (a 1 at sequence position 0), constant
    /// volume 15 and a length, so `pulse1_output` is 15 unless muted.
    fn audible_pulse1(apu: &mut Apu, period: u16) {
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4000, 0xDF);
        apu.write_register(0x4002, (period & 0xFF) as u8);
        apu.write_register(0x4003, ((period >> 8) as u8 & 0x07) | 0x08);
        run(apu, 1);
    }

    /// Pulse 1 output at sequence position 0, so the duty cycle does not
    /// depend on how many cycles the test has stepped.
    fn pulse1_output(apu: &mut Apu) -> u8 {
        apu.pulse1.sequence_pos = 0;
        apu.pulse1._get_output()
    }

    #[test]
    fn sweep_target_period_both_negate_modes() {
        let mut apu = Apu::new();
        apu.write_register(0x4002, 0x00);
        apu.write_register(0x4003, 0x01); // 0x100
        apu.write_register(0x4006, 0x00);
        apu.write_register(0x4007, 0x01);

        // Add mode, shift 2: 0x100 + 0x40.
        apu.write_register(0x4001, 0x02);
        apu.write_register(0x4005, 0x02);
        assert_eq!(apu.pulse1.sweep_target(), 0x140);
        assert_eq!(apu.pulse2.sweep_target(), 0x140);

        // Negate, shift 2: pulse 1 subtracts one extra (one's complement).
        apu.write_register(0x4001, 0x0A);
        apu.write_register(0x4005, 0x0A);
        assert_eq!(apu.pulse1.sweep_target(), 0x100 - 0x40 - 1);
        assert_eq!(apu.pulse2.sweep_target(), 0x100 - 0x40);

        // Negate, shift 0: the change is the whole period, so pulse 2's
        // target is 0 and pulse 1's would be -1, clamped to 0 (not muted).
        apu.write_register(0x4001, 0x08);
        apu.write_register(0x4005, 0x08);
        assert_eq!(apu.pulse1.sweep_target(), 0);
        assert_eq!(apu.pulse2.sweep_target(), 0);
        assert!(!apu.pulse1.sweep_muted());

        // Small period, negate, shift 1: 8 - 4 - 1 and 8 - 4.
        apu.write_register(0x4002, 0x08);
        apu.write_register(0x4003, 0x00);
        apu.write_register(0x4006, 0x08);
        apu.write_register(0x4007, 0x00);
        apu.write_register(0x4001, 0x09);
        apu.write_register(0x4005, 0x09);
        assert_eq!(apu.pulse1.sweep_target(), 3);
        assert_eq!(apu.pulse2.sweep_target(), 4);
    }

    #[test]
    fn sweep_mutes_on_low_period_and_high_target() {
        let mut apu = Apu::new();
        audible_pulse1(&mut apu, 0x100);
        assert_eq!(pulse1_output(&mut apu), 15, "baseline audible");

        apu.write_register(0x4002, 0x07);
        apu.write_register(0x4003, 0x08); // period 7
        run(&mut apu, 1);
        assert!(apu.pulse1.sweep_muted());
        assert_eq!(pulse1_output(&mut apu), 0, "period below 8 mutes");
        apu.write_register(0x4002, 0x08); // period 8
        assert!(!apu.pulse1.sweep_muted());
        assert_eq!(pulse1_output(&mut apu), 15);

        // Period 0x700, add mode, shift 1: target 0xA80 exceeds 0x7FF.
        // The sweep is disabled (bit 7 clear) yet still mutes.
        apu.write_register(0x4002, 0x00);
        apu.write_register(0x4003, 0x07 | 0x08);
        run(&mut apu, 1);
        apu.write_register(0x4001, 0x01);
        assert_eq!(apu.pulse1.sweep_target(), 0xA80);
        assert!(apu.pulse1.sweep_muted());
        assert_eq!(pulse1_output(&mut apu), 0, "target above 0x7FF mutes");

        // Negate mode brings the target back in range: audible again.
        apu.write_register(0x4001, 0x09);
        assert!(!apu.pulse1.sweep_muted());
        assert_eq!(pulse1_output(&mut apu), 15);

        // Muting does not change the period on a clock even when enabled.
        apu.write_register(0x4001, 0x81);
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.timer_period, 0x700);
    }

    /// Enabled, divider period 0, negate, shift 1: each half-frame halves
    /// the period, with pulse 1 one lower than pulse 2.
    #[test]
    fn sweep_down_sequence_on_half_frame_clocks() {
        let mut apu = Apu::new();
        apu.write_register(0x4002, 0x00);
        apu.write_register(0x4003, 0x01);
        apu.write_register(0x4006, 0x00);
        apu.write_register(0x4007, 0x01);
        apu.write_register(0x4001, 0x89);
        apu.write_register(0x4005, 0x89);

        let expected1 = [0x7F, 0x3F, 0x1F, 0x0F];
        let expected2 = [0x80, 0x40, 0x20, 0x10];
        for i in 0..4 {
            write_4017(&mut apu, 0x80); // one half-frame clock
            assert_eq!(apu.pulse1.timer_period, expected1[i], "pulse 1 clock {}", i);
            assert_eq!(apu.pulse2.timer_period, expected2[i], "pulse 2 clock {}", i);
        }

        // Once the period drops below 8 the channel mutes and stops moving.
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.timer_period, 0x07);
        assert!(apu.pulse1.sweep_muted());
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.timer_period, 0x07);
    }

    /// The divider only lets the period change when it is 0; a `$4001`
    /// write reloads it on the next clock.
    #[test]
    fn sweep_divider_reload() {
        let mut apu = Apu::new();
        apu.write_register(0x4002, 0x00);
        apu.write_register(0x4003, 0x01); // 0x100
        apu.write_register(0x4001, 0xA1); // enabled, P=2, add, shift 1

        write_4017(&mut apu, 0x80); // divider 0: update, reload to 2
        assert_eq!(apu.pulse1.timer_period, 0x180);
        assert_eq!(apu.pulse1.sweep_divider, 2);
        write_4017(&mut apu, 0x80); // 2 -> 1
        assert_eq!(apu.pulse1.timer_period, 0x180);
        write_4017(&mut apu, 0x80); // 1 -> 0
        assert_eq!(apu.pulse1.timer_period, 0x180);
        assert_eq!(apu.pulse1.sweep_divider, 0);
        write_4017(&mut apu, 0x80); // 0: update again
        assert_eq!(apu.pulse1.timer_period, 0x240);
        assert_eq!(apu.pulse1.sweep_divider, 2);

        // A register write while the divider is mid-count reloads it on the
        // next clock without changing the period on that clock.
        write_4017(&mut apu, 0x80); // 2 -> 1
        apu.write_register(0x4001, 0xB1); // P=3, reload pending
        assert!(apu.pulse1.sweep_reload);
        write_4017(&mut apu, 0x80);
        assert_eq!(apu.pulse1.timer_period, 0x240);
        assert_eq!(apu.pulse1.sweep_divider, 3);
        assert!(!apu.pulse1.sweep_reload);

        // Shift 0 never updates the period, even with the divider at 0.
        apu.write_register(0x4001, 0x80);
        for _ in 0..3 {
            write_4017(&mut apu, 0x80);
        }
        assert_eq!(apu.pulse1.timer_period, 0x240);
    }
}
