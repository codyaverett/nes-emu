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
            15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0,
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
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

pub struct Dmc {
    enabled: bool,
    rate: u8,
    direct_load: u8,
    sample_address: u16,
    sample_length: u16,
    _current_address: u16,
    bytes_remaining: u16,
    _sample_buffer: Option<u8>,
    output_level: u8,
    _shift_register: u8,
    _bits_remaining: u8,
    _silence_flag: bool,
    irq_enabled: bool,
    loop_flag: bool,
    interrupt: bool,
}

impl Dmc {
    fn new() -> Self {
        Dmc {
            enabled: false,
            rate: 0,
            direct_load: 0,
            sample_address: 0,
            sample_length: 0,
            _current_address: 0,
            bytes_remaining: 0,
            _sample_buffer: None,
            output_level: 0,
            _shift_register: 0,
            _bits_remaining: 0,
            _silence_flag: true,
            irq_enabled: false,
            loop_flag: false,
            interrupt: false,
        }
    }

    fn _get_output(&self) -> u8 {
        self.output_level
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
    frame_interrupt: bool,
    frame_interrupt_inhibit: bool,
    cycles: u64,
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
        self.frame_interrupt = false;
        self.frame_interrupt_inhibit = false;
        self.cycles = 0;
    }

    pub fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x4015 => {
                let mut result = 0u8;
                if self.pulse1.length_counter > 0 { result |= 0x01; }
                if self.pulse2.length_counter > 0 { result |= 0x02; }
                if self.triangle.length_counter > 0 { result |= 0x04; }
                if self.noise.length_counter > 0 { result |= 0x08; }
                if self.dmc.bytes_remaining > 0 { result |= 0x10; }
                if self.frame_interrupt { result |= 0x40; }
                if self.dmc.interrupt { result |= 0x80; }
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
                self.pulse1.timer_period = (self.pulse1.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
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
                self.pulse2.timer_period = (self.pulse2.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
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
                self.triangle.timer_period = (self.triangle.timer_period & 0x00FF) | ((value as u16 & 0x07) << 8);
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
                
                if !self.pulse1.enabled { self.pulse1.length_counter = 0; }
                if !self.pulse2.enabled { self.pulse2.length_counter = 0; }
                if !self.triangle.enabled { self.triangle.length_counter = 0; }
                if !self.noise.enabled { self.noise.length_counter = 0; }
                
                self.dmc.interrupt = false;
            }
            
            0x4017 => {
                self.frame_counter = value;
                self.frame_interrupt_inhibit = (value & 0x40) != 0;
                if self.frame_interrupt_inhibit {
                    self.frame_interrupt = false;
                }
            }
            
            _ => {}
        }
    }

    pub fn step(&mut self) {
        if self.cycles % 2 == 0 {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
            self.noise.clock_timer();
        }
        
        self.triangle.clock_timer();
        
        if self.cycles % 7457 == 0 {
            self.clock_frame_counter();
        }
        
        self.cycles += 1;
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
                if !mode && !self.frame_interrupt_inhibit {
                    self.frame_interrupt = true;
                }
            }
            4 => {
                if mode {
                    self.clock_envelopes();
                    self.clock_linear_counter();
                    self.clock_length_counters();
                    self.clock_sweeps();
                }
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

    fn clock_sweeps(&mut self) {
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
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14,
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

const NOISE_PERIOD_TABLE: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];