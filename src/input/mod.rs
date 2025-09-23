use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct ControllerButton: u8 {
        const A = 0x80;
        const B = 0x40;
        const SELECT = 0x20;
        const START = 0x10;
        const UP = 0x08;
        const DOWN = 0x04;
        const LEFT = 0x02;
        const RIGHT = 0x01;
    }
}

pub struct Controller {
    buttons: ControllerButton,
    strobe: bool,
    index: u8,
}

impl Controller {
    pub fn new() -> Self {
        Controller {
            buttons: ControllerButton::empty(),
            strobe: false,
            index: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buttons = ControllerButton::empty();
        self.strobe = false;
        self.index = 0;
    }

    pub fn _set_button(&mut self, button: ControllerButton, pressed: bool) {
        if pressed {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(button);
        }
    }

    pub fn write(&mut self, value: u8) {
        let was_strobe = self.strobe;
        self.strobe = (value & 0x01) != 0;
        
        // Reset index when strobe goes from high to low
        if was_strobe && !self.strobe {
            self.index = 0;
            log::debug!("Controller strobe reset (high->low), ready to read buttons");
        }
        
        log::trace!("Controller strobe write: value={:02X}, strobe={}, was_strobe={}", value, self.strobe, was_strobe);
    }

    pub fn read(&mut self) -> u8 {
        // The order that NES reads controller buttons: A, B, Select, Start, Up, Down, Left, Right
        let button_order = [
            ControllerButton::A,
            ControllerButton::B,
            ControllerButton::SELECT,
            ControllerButton::START,
            ControllerButton::UP,
            ControllerButton::DOWN,
            ControllerButton::LEFT,
            ControllerButton::RIGHT,
        ];

        let button_state = if self.strobe {
            // When strobe is high, always return A button state
            self.buttons.contains(ControllerButton::A)
        } else if self.index < 8 {
            // When strobe is low, return button states in sequence
            self.buttons.contains(button_order[self.index as usize])
        } else {
            // After 8 reads, return open bus (traditionally reads as 1)
            true
        };

        let result = if button_state { 0x01 } else { 0x00 };
        
        // Always log the first read after strobe reset
        if self.index == 0 && !self.strobe {
            log::debug!("Controller first read after strobe: buttons={:08b}, A pressed={}", 
                       self.buttons.bits(), self.buttons.contains(ControllerButton::A));
        }
        
        // Log any non-zero button state
        if result != 0 || self.buttons.bits() != 0 {
            log::debug!("Controller read: strobe={}, index={}, buttons={:08b}, result={:02X}", 
                       self.strobe, self.index, self.buttons.bits(), result);
        }

        // Only increment index when strobe is low and we haven't read all 8 buttons yet
        if !self.strobe && self.index < 8 {
            self.index += 1;
        }

        result
    }

    pub fn press(&mut self, button: ControllerButton) {
        self.buttons.insert(button);
        log::info!("Button pressed: {:?}, state: {:08b}", button, self.buttons.bits());
    }

    pub fn release(&mut self, button: ControllerButton) {
        self.buttons.remove(button);
        log::info!("Button released: {:?}, state: {:08b}", button, self.buttons.bits());
    }

    pub fn _is_pressed(&self, button: ControllerButton) -> bool {
        self.buttons.contains(button)
    }
}