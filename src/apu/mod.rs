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

pub struct Pulse {
    enabled: bool,
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
    _sweep_counter: u8,
    timer_period: u16,
    timer_counter: u16,
    length_counter: u8,
    sequence_pos: u8,
}

impl Pulse {
    fn new() -> Self {
        Pulse {
            enabled: false,
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
            _sweep_counter: 0,
            timer_period: 0,
            timer_counter: 0,
            length_counter: 0,
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

    fn _get_output(&self) -> u8 {
        if !self.enabled || self.length_counter == 0 || self.timer_period < 8 {
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
    enabled: bool,
    linear_counter: u8,
    linear_counter_period: u8,
    linear_counter_reload: bool,
    timer_period: u16,
    timer_counter: u16,
    length_counter: u8,
    sequence_pos: u8,
}

impl Triangle {
    fn new() -> Self {
        Triangle {
            enabled: false,
            linear_counter: 0,
            linear_counter_period: 0,
            linear_counter_reload: false,
            timer_period: 0,
            timer_counter: 0,
            length_counter: 0,
            sequence_pos: 0,
        }
    }

    fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer_period;
            if self.linear_counter > 0 && self.length_counter > 0 {
                self.sequence_pos = (self.sequence_pos + 1) % 32;
            }
        } else {
            self.timer_counter -= 1;
        }
    }

    fn _get_output(&self) -> u8 {
        if !self.enabled || self.length_counter == 0 || self.linear_counter == 0 {
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
    enabled: bool,
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
    length_counter: u8,
    shift_register: u16,
}

impl Noise {
    fn new() -> Self {
        Noise {
            enabled: false,
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
            length_counter: 0,
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
        if !self.enabled || self.length_counter == 0 || (self.shift_register & 1) == 1 {
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

/// Approximate CPU cycles between frame sequencer steps (NTSC: the 4-step
/// sequence runs at ~240 Hz, i.e. 29830 CPU cycles per 4 steps).
const FRAME_STEP_CYCLES: u32 = 7457;

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
    frame_counter: u8,
    frame_sequence: u8,
    /// CPU cycles until the next frame sequencer step. Separate from
    /// `cycles` so a $4017 write can reset the sequencer without disturbing
    /// the channel timer parity.
    frame_divider: u32,
    /// Frame interrupt flag: set on the last step of the 4-step sequence
    /// unless inhibited. Held until a $4015 read or a $4017 write with
    /// bit 6 (inhibit) set.
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
            pulse1: Pulse::new(),
            pulse2: Pulse::new(),
            triangle: Triangle::new(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            status: ApuStatus::empty(),
            frame_counter: 0,
            frame_sequence: 0,
            frame_divider: FRAME_STEP_CYCLES,
            frame_interrupt: false,
            frame_interrupt_inhibit: false,
            cycles: 0,
        }
    }

    pub fn reset(&mut self) {
        self.pulse1 = Pulse::new();
        self.pulse2 = Pulse::new();
        self.triangle = Triangle::new();
        self.noise = Noise::new();
        self.dmc = Dmc::new();
        self.status = ApuStatus::empty();
        self.frame_counter = 0;
        self.frame_sequence = 0;
        self.frame_divider = FRAME_STEP_CYCLES;
        self.frame_interrupt = false;
        self.frame_interrupt_inhibit = false;
        self.cycles = 0;
    }

    pub fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x4015 => {
                let mut result = 0u8;
                if self.pulse1.length_counter > 0 {
                    result |= 0x01;
                }
                if self.pulse2.length_counter > 0 {
                    result |= 0x02;
                }
                if self.triangle.length_counter > 0 {
                    result |= 0x04;
                }
                if self.noise.length_counter > 0 {
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
                self.pulse1.constant_volume = (value & 0x10) != 0;
                self.pulse1.volume = value & 0x0F;
                self.pulse1.envelope_period = value & 0x0F;
            }
            0x4001 => {
                self.pulse1.sweep_enabled = (value & 0x80) != 0;
                self.pulse1.sweep_period = (value >> 4) & 0x07;
                self.pulse1.sweep_negate = (value & 0x08) != 0;
                self.pulse1.sweep_shift = value & 0x07;
            }
            0x4002 => {
                self.pulse1.timer_period = (self.pulse1.timer_period & 0xFF00) | value as u16;
            }
            0x4003 => {
                self.pulse1.timer_period =
                    (self.pulse1.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.pulse1.length_counter = LENGTH_TABLE[(value >> 3) as usize];
                self.pulse1.envelope_start = true;
            }

            0x4004 => {
                self.pulse2.duty = (value >> 6) & 0x03;
                self.pulse2.envelope_loop = (value & 0x20) != 0;
                self.pulse2.constant_volume = (value & 0x10) != 0;
                self.pulse2.volume = value & 0x0F;
                self.pulse2.envelope_period = value & 0x0F;
            }
            0x4005 => {
                self.pulse2.sweep_enabled = (value & 0x80) != 0;
                self.pulse2.sweep_period = (value >> 4) & 0x07;
                self.pulse2.sweep_negate = (value & 0x08) != 0;
                self.pulse2.sweep_shift = value & 0x07;
            }
            0x4006 => {
                self.pulse2.timer_period = (self.pulse2.timer_period & 0xFF00) | value as u16;
            }
            0x4007 => {
                self.pulse2.timer_period =
                    (self.pulse2.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.pulse2.length_counter = LENGTH_TABLE[(value >> 3) as usize];
                self.pulse2.envelope_start = true;
            }

            0x4008 => {
                self.triangle.linear_counter_period = value & 0x7F;
            }
            0x400A => {
                self.triangle.timer_period = (self.triangle.timer_period & 0xFF00) | value as u16;
            }
            0x400B => {
                self.triangle.timer_period =
                    (self.triangle.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
                self.triangle.length_counter = LENGTH_TABLE[(value >> 3) as usize];
                self.triangle.linear_counter_reload = true;
            }

            0x400C => {
                self.noise.envelope_loop = (value & 0x20) != 0;
                self.noise.constant_volume = (value & 0x10) != 0;
                self.noise.volume = value & 0x0F;
                self.noise.envelope_period = value & 0x0F;
            }
            0x400E => {
                self.noise.mode = (value & 0x80) != 0;
                self.noise.timer_period = NOISE_PERIOD_TABLE[(value & 0x0F) as usize];
            }
            0x400F => {
                self.noise.length_counter = LENGTH_TABLE[(value >> 3) as usize];
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
                self.pulse1.enabled = (value & 0x01) != 0;
                self.pulse2.enabled = (value & 0x02) != 0;
                self.triangle.enabled = (value & 0x04) != 0;
                self.noise.enabled = (value & 0x08) != 0;
                self.dmc.enabled = (value & 0x10) != 0;

                if !self.pulse1.enabled {
                    self.pulse1.length_counter = 0;
                }
                if !self.pulse2.enabled {
                    self.pulse2.length_counter = 0;
                }
                if !self.triangle.enabled {
                    self.triangle.length_counter = 0;
                }
                if !self.noise.enabled {
                    self.noise.length_counter = 0;
                }

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

                // Writing $4017 resets the frame sequencer. In 5-step mode
                // the quarter- and half-frame units are clocked immediately.
                // (Hardware applies the reset 3-4 CPU cycles after the write;
                // that delay is not modelled yet.)
                self.frame_sequence = 0;
                self.frame_divider = FRAME_STEP_CYCLES;
                if (value & 0x80) != 0 {
                    self.clock_envelopes();
                    self.clock_linear_counter();
                    self.clock_length_counters();
                    self.clock_sweeps();
                }
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

        self.frame_divider -= 1;
        if self.frame_divider == 0 {
            self.frame_divider = FRAME_STEP_CYCLES;
            self.clock_frame_counter();
        }

        self.cycles += 1;
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

    fn clock_frame_counter(&mut self) {
        let mode = (self.frame_counter & 0x80) != 0;

        match self.frame_sequence {
            0 | 2 => {
                self.clock_envelopes();
                self.clock_linear_counter();
            }
            1 => {
                self.clock_envelopes();
                self.clock_linear_counter();
                self.clock_length_counters();
                self.clock_sweeps();
            }
            3 => {
                self.clock_envelopes();
                self.clock_linear_counter();
                self.clock_length_counters();
                self.clock_sweeps();
                // 4-step mode: the last step raises the frame interrupt
                // unless inhibited by $4017 bit 6.
                if !mode && !self.frame_interrupt_inhibit {
                    self.frame_interrupt = true;
                }
            }
            4 if mode => {
                self.clock_envelopes();
                self.clock_linear_counter();
                self.clock_length_counters();
                self.clock_sweeps();
            }
            _ => {}
        }

        self.frame_sequence = if mode {
            (self.frame_sequence + 1) % 5
        } else {
            (self.frame_sequence + 1) % 4
        };
    }

    fn clock_envelopes(&mut self) {
        self.pulse1.clock_envelope();
        self.pulse2.clock_envelope();
        self.noise.clock_envelope();
    }

    fn clock_linear_counter(&mut self) {
        if self.triangle.linear_counter_reload {
            self.triangle.linear_counter = self.triangle.linear_counter_period;
        } else if self.triangle.linear_counter > 0 {
            self.triangle.linear_counter -= 1;
        }
    }

    fn clock_length_counters(&mut self) {
        if self.pulse1.length_counter > 0 {
            self.pulse1.length_counter -= 1;
        }
        if self.pulse2.length_counter > 0 {
            self.pulse2.length_counter -= 1;
        }
        if self.triangle.length_counter > 0 {
            self.triangle.length_counter -= 1;
        }
        if self.noise.length_counter > 0 {
            self.noise.length_counter -= 1;
        }
    }

    fn clock_sweeps(&mut self) {}

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

    #[test]
    fn frame_irq_set_on_last_step_of_4_step_mode() {
        let mut apu = Apu::new();
        apu.write_register(0x4017, 0x00);
        run(&mut apu, FRAME_STEP_CYCLES * 3);
        assert!(!apu.irq_pending(), "not before the fourth step");
        run(&mut apu, FRAME_STEP_CYCLES);
        assert!(apu.irq_pending(), "set on the fourth step");
        assert_ne!(apu.read_register(0x4015) & 0x40, 0);
        assert!(!apu.irq_pending(), "$4015 read clears the frame flag");
        assert_eq!(apu.read_register(0x4015) & 0x40, 0);
    }

    #[test]
    fn frame_irq_held_until_acknowledged() {
        let mut apu = Apu::new();
        apu.write_register(0x4017, 0x00);
        run(&mut apu, FRAME_STEP_CYCLES * 4);
        assert!(apu.irq_pending());
        run(&mut apu, FRAME_STEP_CYCLES * 3);
        assert!(apu.irq_pending(), "flag is held, not pulsed");
    }

    #[test]
    fn frame_irq_inhibited_and_cleared_by_4017_bit6() {
        let mut apu = Apu::new();
        apu.write_register(0x4017, 0x40);
        run(&mut apu, FRAME_STEP_CYCLES * 8);
        assert!(!apu.irq_pending(), "inhibit bit blocks the flag");

        apu.write_register(0x4017, 0x00);
        run(&mut apu, FRAME_STEP_CYCLES * 4);
        assert!(apu.irq_pending());
        apu.write_register(0x4017, 0x40);
        assert!(!apu.irq_pending(), "writing $4017 with inhibit clears it");
    }

    #[test]
    fn frame_irq_never_set_in_5_step_mode() {
        let mut apu = Apu::new();
        apu.write_register(0x4017, 0x80);
        run(&mut apu, FRAME_STEP_CYCLES * 10);
        assert!(!apu.irq_pending());
    }

    #[test]
    fn write_4017_resets_sequencer() {
        let mut apu = Apu::new();
        apu.write_register(0x4017, 0x00);
        run(&mut apu, FRAME_STEP_CYCLES * 3 + 100);
        // Reset: the 4th step is now four full periods away again.
        apu.write_register(0x4017, 0x00);
        run(&mut apu, FRAME_STEP_CYCLES * 4 - 1);
        assert!(!apu.irq_pending());
        run(&mut apu, 1);
        assert!(apu.irq_pending());
    }

    #[test]
    fn write_4017_5_step_clocks_length_counters_immediately() {
        let mut apu = Apu::new();
        apu.write_register(0x4015, 0x01);
        apu.write_register(0x4003, 0x08); // length index 1 -> 254
        let before = apu.pulse1.length_counter;
        apu.write_register(0x4017, 0x80);
        assert_eq!(apu.pulse1.length_counter, before - 1);
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
}
