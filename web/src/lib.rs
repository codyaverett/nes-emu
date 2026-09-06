//! wasm-bindgen wrapper around the `nes-emu` core (issue #49,
//! docs/plans/WASM_WEB.md).
//!
//! The page owns pacing, persistence and input mapping; this crate only
//! exposes the core as one `Emulator` object with byte and string APIs.
//! Every fallible method returns `Result<_, String>` rather than
//! `JsValue` so the same code runs in the host-side unit tests (creating a
//! `JsValue` outside wasm panics).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use nes_emu::cartridge::Cartridge;
use nes_emu::cheat::CheatSet;
use nes_emu::input::ControllerButton;
use nes_emu::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use nes_emu::system::System;
use wasm_bindgen::prelude::*;
use wasm_bindgen::Clamped;

/// Frame width in pixels, before any overscan crop.
pub const FRAME_WIDTH: u32 = SCREEN_WIDTH as u32;
/// Frame height in pixels, before any overscan crop.
pub const FRAME_HEIGHT: u32 = SCREEN_HEIGHT as u32;
/// Rate of the samples `take_audio` returns, matching `System`'s mixer.
pub const AUDIO_SAMPLE_RATE: u32 = 44_100;

/// Button bit layout, identical to `ControllerButton` and to the order
/// the page uses: A, B, Select, Start, Up, Down, Left, Right.
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Up = 4,
    Down = 5,
    Left = 6,
    Right = 7,
}

impl Button {
    fn flag(self) -> ControllerButton {
        match self {
            Button::A => ControllerButton::A,
            Button::B => ControllerButton::B,
            Button::Select => ControllerButton::SELECT,
            Button::Start => ControllerButton::START,
            Button::Up => ControllerButton::UP,
            Button::Down => ControllerButton::DOWN,
            Button::Left => ControllerButton::LEFT,
            Button::Right => ControllerButton::RIGHT,
        }
    }
}

#[wasm_bindgen]
pub struct Emulator {
    system: System,
    audio: Arc<Mutex<VecDeque<f32>>>,
    rgba: Vec<u8>,
    rom_crc32: u32,
    mapper_id: u8,
    battery_backed: bool,
}

#[wasm_bindgen]
impl Emulator {
    /// Load an iNES image. Errors carry the loader's message (bad header,
    /// unsupported mapper).
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, String> {
        let cartridge = Cartridge::load_from_bytes(rom).map_err(|e| e.to_string())?;
        let rom_crc32 = cartridge.rom_crc32;
        let mapper_id = cartridge.mapper_id;
        let battery_backed = cartridge.battery_backed;
        let mut system = System::new();
        system.load_cartridge(cartridge);
        Ok(Emulator {
            system,
            audio: Arc::new(Mutex::new(VecDeque::new())),
            rgba: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            rom_crc32,
            mapper_id,
            battery_backed,
        })
    }

    /// Emulate one frame, capturing audio for `take_audio`.
    pub fn run_frame(&mut self) {
        self.system.run_frame_with_audio(Some(&self.audio));
    }

    /// The last frame as 256x240 RGBA with opaque alpha, ready for
    /// `ImageData`. The page applies the overscan crop when it draws.
    pub fn frame_rgba(&mut self) -> Clamped<Vec<u8>> {
        let rgb = self.system.get_frame_buffer();
        for (dst, src) in self.rgba.chunks_exact_mut(4).zip(rgb.chunks_exact(3)) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = 0xFF;
        }
        Clamped(self.rgba.clone())
    }

    /// Drain every sample produced since the last call (44.1 kHz mono,
    /// roughly -1..1). Call once per `run_frame` batch.
    pub fn take_audio(&mut self) -> Vec<f32> {
        let mut queue = self.audio.lock().unwrap();
        queue.drain(..).collect()
    }

    /// Samples waiting in the queue, for pacing decisions.
    pub fn queued_audio(&self) -> usize {
        self.audio.lock().unwrap().len()
    }

    /// Press or release a button on controller 1 (`player` 0) or 2.
    pub fn set_button(&mut self, player: u8, button: Button, down: bool) {
        let controller = if player == 0 {
            &mut self.system.controller1
        } else {
            &mut self.system.controller2
        };
        if down {
            controller.press(button.flag());
        } else {
            controller.release(button.flag());
        }
    }

    /// Release every button on both controllers (window blur).
    pub fn release_all(&mut self) {
        self.system.controller1.reset();
        self.system.controller2.reset();
    }

    /// Press the console's reset button.
    pub fn reset(&mut self) {
        self.system.reset();
        self.audio.lock().unwrap().clear();
    }

    /// Whole-machine save state in the NESS format.
    pub fn save_state(&self) -> Vec<u8> {
        self.system.save_state()
    }

    /// Restore a state produced by `save_state` for the same ROM.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        self.system.load_state(data).map_err(|e| e.to_string())?;
        self.audio.lock().unwrap().clear();
        Ok(())
    }

    /// Battery-backed PRG RAM, or `None` for boards without a battery.
    pub fn battery(&self) -> Option<Vec<u8>> {
        self.system.battery_ram().map(|ram| ram.to_vec())
    }

    /// Restore battery RAM; `false` if the board has none or the size
    /// does not match.
    pub fn set_battery(&mut self, data: &[u8]) -> bool {
        self.system.set_battery_ram(data)
    }

    /// True when battery RAM changed since the last `set_battery` or
    /// `mark_battery_saved`.
    pub fn battery_dirty(&self) -> bool {
        self.system.battery_dirty()
    }

    pub fn mark_battery_saved(&self) {
        self.system.mark_battery_saved();
    }

    /// The active cheat set in `.cht` text form.
    pub fn cheats_text(&self) -> String {
        self.system.cheats().to_string()
    }

    /// Replace the cheat set from `.cht` text (one code per line, tab
    /// separated enabled flag and description, `#` comments).
    pub fn set_cheats_text(&mut self, text: &str) -> Result<(), String> {
        let set = CheatSet::parse(text).map_err(|e| e.to_string())?;
        *self.system.cheats_mut() = set;
        Ok(())
    }

    /// CRC-32 of the ROM image after the header; the key for saves,
    /// states and the bundled cheat database.
    pub fn rom_crc32(&self) -> u32 {
        self.rom_crc32
    }

    pub fn mapper_id(&self) -> u8 {
        self.mapper_id
    }

    pub fn battery_backed(&self) -> bool {
        self.battery_backed
    }

    pub fn frame_width(&self) -> u32 {
        FRAME_WIDTH
    }

    pub fn frame_height(&self) -> u32 {
        FRAME_HEIGHT
    }

    pub fn audio_sample_rate(&self) -> u32 {
        AUDIO_SAMPLE_RATE
    }
}

/// Emulator core version, for stamping browser-side state records.
#[wasm_bindgen]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16-byte header plus 32 KB of NOPs with a reset vector at $8000.
    fn synthetic_rom(battery: bool) -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 0x8000);
        rom.extend_from_slice(b"NES\x1A");
        rom.push(2);
        rom.push(0);
        rom.push(if battery { 0x02 } else { 0x00 });
        rom.push(0);
        rom.extend_from_slice(&[0; 8]);
        let mut prg = vec![0xEA; 0x8000];
        prg[0x7FFC] = 0x00;
        prg[0x7FFD] = 0x80;
        rom.extend_from_slice(&prg);
        rom
    }

    #[test]
    fn rejects_garbage() {
        assert!(Emulator::new(b"not a rom").is_err());
    }

    #[test]
    fn runs_sixty_frames_with_video_and_audio() {
        let mut emu = Emulator::new(&synthetic_rom(false)).unwrap();
        assert_eq!(emu.mapper_id(), 0);
        assert!(!emu.battery_backed());
        assert!(emu.battery().is_none());
        for _ in 0..60 {
            emu.run_frame();
        }
        let frame = emu.frame_rgba();
        assert_eq!(frame.len(), 256 * 240 * 4);
        assert!(frame.iter().skip(3).step_by(4).all(|&a| a == 0xFF));
        let queued = emu.queued_audio();
        // 60 frames at 44.1 kHz is about 44 100 samples; the queue caps at
        // 8192 so run_frame drops the rest.
        assert!(queued > 4000, "queued {queued}");
        let audio = emu.take_audio();
        assert_eq!(audio.len(), queued);
        assert_eq!(emu.queued_audio(), 0);
    }

    #[test]
    fn state_and_cheats_round_trip() {
        let mut emu = Emulator::new(&synthetic_rom(true)).unwrap();
        assert!(emu.battery_backed());
        assert_eq!(emu.battery().unwrap().len(), 0x2000);
        emu.run_frame();
        let state = emu.save_state();
        assert!(state.starts_with(b"NESS"));
        emu.run_frame();
        emu.load_state(&state).unwrap();
        assert!(emu.load_state(b"junk").is_err());

        emu.set_cheats_text("SXIOPO\t1\tinfinite lives\n").unwrap();
        assert!(emu.cheats_text().contains("SXIOPO"));
        assert!(emu.set_cheats_text("NOTACODE!\t1\tx\n").is_err());

        let ram = vec![0xAB; 0x2000];
        assert!(emu.set_battery(&ram));
        assert!(!emu.battery_dirty());
        assert_eq!(emu.battery().unwrap()[0x100], 0xAB);
        assert!(!emu.set_battery(&ram[..1]));
        assert_ne!(emu.rom_crc32(), 0);
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}
