//! wasm-bindgen wrapper around the `nes-emu` core (issue #49,
//! docs/plans/WASM_WEB.md) and the shared overlay UI (issue #52,
//! docs/plans/SHARED_OVERLAY_UI.md).
//!
//! The page owns pacing, persistence and the controller mapping; this
//! crate exposes one `Emulator` object that holds the library's `App`
//! (run-control flags, slots, cheats, rewind, toasts), the `Ui` (command
//! palette and tool pages) and an `RgbaPainter` the overlay is drawn
//! into. Keys go through `key_down` before the page's controller table,
//! exactly as in the SDL binary's `main.rs`. Every fallible method
//! returns `Result<_, String>` rather than `JsValue` so the same code runs
//! in the host-side unit tests (creating a `JsValue` outside wasm panics).

mod host;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use nes_emu::cartridge::Cartridge;
use nes_emu::cheat::CheatSet;
use nes_emu::input::ControllerButton;
use nes_emu::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use nes_emu::system::System;
use nes_emu::ui::app::App;
use nes_emu::ui::key::Key;
use nes_emu::ui::painter::{Painter, RgbaPainter};
use nes_emu::ui::{self, Ui};
use wasm_bindgen::prelude::*;
use wasm_bindgen::Clamped;

use host::{SharedStore, SlotStore, WebHost};

/// Frame width in pixels, before any overscan crop.
pub const FRAME_WIDTH: u32 = SCREEN_WIDTH as u32;
/// Frame height in pixels, before any overscan crop.
pub const FRAME_HEIGHT: u32 = SCREEN_HEIGHT as u32;
/// Rate of the samples `take_audio` returns, matching `System`'s mixer.
pub const AUDIO_SAMPLE_RATE: u32 = 44_100;
/// Starting volume: the page has always played the mixer at unity gain.
const INITIAL_VOLUME: f32 = 1.0;

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
    app: App,
    ui: Ui,
    painter: RgbaPainter,
    store: SharedStore,
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
        let store: SharedStore = Rc::new(RefCell::new(SlotStore::new()));
        let app = App::new(
            system,
            Some(Arc::new(Mutex::new(VecDeque::new()))),
            Arc::new(Mutex::new(false)),
            Arc::new(Mutex::new(INITIAL_VOLUME)),
            true,
            Box::new(WebHost::new(Rc::clone(&store))),
        );
        let (w, h) = app.visible_size();
        let (ow, oh) = ui::overlay_size(w, h);
        Ok(Emulator {
            app,
            ui: Ui::new(ui::DEFAULT_FONT_SCALE),
            painter: RgbaPainter::new(ow, oh),
            store,
            rgba: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            rom_crc32,
            mapper_id,
            battery_backed,
        })
    }

    // ---------------------------------------------------------- frames

    /// Emulate one frame, capturing audio for `take_audio` and recording
    /// a rewind snapshot when one is due.
    pub fn run_frame(&mut self) {
        let buffer = self.app.audio_buffer.clone();
        self.app.system.run_frame_with_audio(buffer.as_ref());
        self.app.record_rewind_frame();
    }

    /// The last frame as 256x240 RGBA with opaque alpha, ready for
    /// `ImageData`. The page applies the overscan crop when it draws.
    pub fn frame_rgba(&mut self) -> Clamped<Vec<u8>> {
        let rgb = self.app.system.get_frame_buffer();
        for i in 0..SCREEN_WIDTH * SCREEN_HEIGHT {
            self.rgba[i * 4..i * 4 + 3].copy_from_slice(&rgb[i * 3..i * 3 + 3]);
            self.rgba[i * 4 + 3] = 0xFF;
        }
        Clamped(self.rgba.clone())
    }

    /// Drain every sample produced since the last call (44.1 kHz mono,
    /// roughly -1..1, before the volume and mute the page applies). Call
    /// once per `run_frame` batch.
    pub fn take_audio(&mut self) -> Vec<f32> {
        match &self.app.audio_buffer {
            Some(queue) => queue.lock().unwrap().drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// Samples waiting in the queue, for pacing decisions.
    pub fn queued_audio(&self) -> usize {
        self.app
            .audio_buffer
            .as_ref()
            .map_or(0, |q| q.lock().unwrap().len())
    }

    // ------------------------------------------------------------ input

    /// Press or release a button on controller 1 (`player` 0) or 2.
    pub fn set_button(&mut self, player: u8, button: Button, down: bool) {
        let controller = if player == 0 {
            &mut self.app.system.controller1
        } else {
            &mut self.app.system.controller2
        };
        if down {
            controller.press(button.flag());
        } else {
            controller.release(button.flag());
        }
    }

    /// Release every button on both controllers (window blur).
    pub fn release_all(&mut self) {
        self.app.system.controller1.reset();
        self.app.system.controller2.reset();
    }

    /// A key press by its `KeyboardEvent.code`. The palette or open page
    /// sees it first, then the shared hotkeys (F1, R, P, N, M, plus,
    /// minus, F5-F8, Backspace). Returns true when the key was used: the
    /// page must then `preventDefault` and skip its controller table.
    pub fn key_down(&mut self, code: &str) -> bool {
        self.ui
            .key_down(Key::from_browser_code(code), &mut self.app)
    }

    /// A key release by its `KeyboardEvent.code` (ends a rewind).
    pub fn key_up(&mut self, code: &str) {
        self.ui.key_up(Key::from_browser_code(code), &mut self.app);
    }

    // ------------------------------------------------------------ clock

    /// The UI clock, milliseconds; pass `Date.now()` once per tick before
    /// handling keys and drawing. Toasts expire against it and slot
    /// writes are stamped with it.
    pub fn set_now_ms(&mut self, ms: f64) {
        let ms = ms.max(0.0) as u64;
        self.app.now_ms = ms;
        self.store.borrow_mut().set_now_unix_secs(ms / 1000);
    }

    /// Per-tick housekeeping: the open page's `tick`, and resizing the
    /// overlay after an overscan crop change.
    pub fn tick(&mut self) {
        self.ui.tick(&mut self.app);
        if self.app.crop_dirty {
            self.app.crop_dirty = false;
            let (w, h) = self.app.visible_size();
            let (ow, oh) = ui::overlay_size(w, h);
            self.painter.resize(ow, oh);
        }
    }

    // ---------------------------------------------------------- overlay

    /// `[width, height]` of the overlay in pixels: the visible picture at
    /// `WINDOW_SCALE`, the SDL window size, so the UI draws identically.
    pub fn overlay_size(&self) -> Vec<u32> {
        let (w, h) = self.painter.size();
        vec![w, h]
    }

    /// True when `overlay_rgba` would draw anything: the palette or a
    /// page is open, a toast or the volume bar is showing, or the paused
    /// reminder applies. When false the page clears its overlay canvas.
    pub fn overlay_visible(&self) -> bool {
        self.ui.is_active() || self.app.messages_visible()
    }

    /// Draw the messages and the current palette or page into a fresh
    /// transparent overlay and return it as RGBA, `overlay_size` pixels.
    pub fn overlay_rgba(&mut self) -> Result<Clamped<Vec<u8>>, String> {
        self.painter.clear();
        ui::draw_messages(&mut self.painter, self.ui.font_scale, &self.app)?;
        self.ui.draw(&mut self.painter, &self.app)?;
        Ok(Clamped(self.painter.pixels().to_vec()))
    }

    /// Show a toast through the shared overlay ("Loaded mario.nes").
    pub fn osd_message(&mut self, text: &str) {
        self.app.show_message(text);
    }

    /// The toast currently showing, if any (for scripts; the page has no
    /// toast element to read).
    pub fn osd_text(&self) -> Option<String> {
        self.app.osd_message().map(str::to_owned)
    }

    // ------------------------------------------------------ run control

    pub fn paused(&self) -> bool {
        self.app.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused {
            self.app.pause();
        } else {
            self.app.resume();
        }
    }

    /// True once after a frame advance was requested (N, or the palette
    /// command); the page then runs exactly one frame while paused.
    pub fn take_frame_advance(&mut self) -> bool {
        std::mem::take(&mut self.app.frame_advance)
    }

    pub fn muted(&self) -> bool {
        self.app.is_muted()
    }

    pub fn toggle_mute(&mut self) {
        self.app.toggle_mute();
    }

    /// Volume 0..1; the page applies it to its gain node.
    pub fn volume(&self) -> f32 {
        self.app.volume()
    }

    pub fn crop_enabled(&self) -> bool {
        self.app.crop_enabled
    }

    /// Set the overscan crop (the page's Full frame button).
    pub fn set_crop(&mut self, crop: bool) {
        if self.app.crop_enabled != crop {
            self.app.toggle_crop();
        }
    }

    /// True while the palette or a page owns the keyboard.
    pub fn ui_active(&self) -> bool {
        self.ui.is_active()
    }

    /// Press the console's reset button.
    pub fn reset(&mut self) {
        self.app.reset();
        if let Some(queue) = &self.app.audio_buffer {
            queue.lock().unwrap().clear();
        }
    }

    // ----------------------------------------------------------- rewind

    /// True while Backspace is held: the page calls `rewind_step` once
    /// per display frame instead of emulating.
    pub fn rewinding(&self) -> bool {
        self.app.rewinding
    }

    /// One rewind frame: drops queued audio, loads the newest snapshot
    /// and runs one frame to refresh the picture (`App::rewind_frame`).
    pub fn rewind_step(&mut self) {
        self.app.rewind_frame();
    }

    /// Seconds of gameplay the rewind buffer holds.
    pub fn rewind_seconds(&self) -> f32 {
        self.app.rewind_seconds()
    }

    // ------------------------------------------------------------ slots

    /// Current save-state slot, 1-9.
    pub fn slot(&self) -> u8 {
        self.app.slot
    }

    pub fn set_slot(&mut self, slot: u8) {
        self.app.set_slot(slot);
    }

    /// Save the machine to `slot` (the page's Save button; F5 goes
    /// through `key_down`). The slot then shows up in `take_dirty_slots`.
    pub fn save_slot(&mut self, slot: u8) {
        self.app.save_state_to(slot);
    }

    /// Load `slot` (the page's Load button); an empty slot only toasts.
    pub fn load_slot(&mut self, slot: u8) {
        self.app.load_state_from(slot);
        if let Some(queue) = &self.app.audio_buffer {
            queue.lock().unwrap().clear();
        }
    }

    /// Fill the slot cache from the page's store on ROM load, without
    /// marking the slot dirty. `modified_unix_ms` is the store's stamp.
    pub fn set_slot_cache(&mut self, slot: u8, bytes: &[u8], modified_unix_ms: f64) {
        let secs = (modified_unix_ms > 0.0).then(|| (modified_unix_ms / 1000.0) as u64);
        self.store.borrow_mut().set_cached(slot, bytes, secs);
    }

    /// Forget a cached slot (the page deleted it from the store).
    pub fn clear_slot_cache(&mut self, slot: u8) {
        self.store.borrow_mut().clear_cached(slot);
    }

    /// Slots the UI wrote since the last call, 1-based ascending; the
    /// page persists each with `slot_bytes`.
    pub fn take_dirty_slots(&mut self) -> Vec<u8> {
        self.store.borrow_mut().take_dirty()
    }

    /// The cached image of `slot`, or `undefined` when empty.
    pub fn slot_bytes(&self, slot: u8) -> Option<Vec<u8>> {
        self.store.borrow().bytes(slot)
    }

    /// True once after the UI changed the cheat set; the page then
    /// stores `cheats_text`.
    pub fn take_cheats_dirty(&mut self) -> bool {
        self.store.borrow_mut().take_cheats_dirty()
    }

    // --------------------------------------------------- raw state APIs

    /// Whole-machine save state in the NESS format.
    pub fn save_state(&self) -> Vec<u8> {
        self.app.system.save_state()
    }

    /// Restore a state produced by `save_state` for the same ROM.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        self.app
            .system
            .load_state(data)
            .map_err(|e| e.to_string())?;
        if let Some(queue) = &self.app.audio_buffer {
            queue.lock().unwrap().clear();
        }
        Ok(())
    }

    /// Battery-backed PRG RAM, or `None` for boards without a battery.
    pub fn battery(&self) -> Option<Vec<u8>> {
        self.app.system.battery_ram().map(|ram| ram.to_vec())
    }

    /// Restore battery RAM; `false` if the board has none or the size
    /// does not match.
    pub fn set_battery(&mut self, data: &[u8]) -> bool {
        self.app.system.set_battery_ram(data)
    }

    /// True when battery RAM changed since the last `set_battery` or
    /// `mark_battery_saved`.
    pub fn battery_dirty(&self) -> bool {
        self.app.system.battery_dirty()
    }

    pub fn mark_battery_saved(&self) {
        self.app.system.mark_battery_saved();
    }

    /// The active cheat set in `.cht` text form.
    pub fn cheats_text(&self) -> String {
        self.app.system.cheats().to_string()
    }

    /// Replace the cheat set from `.cht` text (one code per line, tab
    /// separated enabled flag and description, `#` comments). Does not
    /// mark the cheats dirty: the page is the one writing them.
    pub fn set_cheats_text(&mut self, text: &str) -> Result<(), String> {
        let set = CheatSet::parse(text).map_err(|e| e.to_string())?;
        *self.app.system.cheats_mut() = set;
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
        // Thirty snapshots at one every two frames.
        assert!((emu.rewind_seconds() - 1.0).abs() < 0.01);
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
        assert!(!emu.take_cheats_dirty());

        let ram = vec![0xAB; 0x2000];
        assert!(emu.set_battery(&ram));
        assert!(!emu.battery_dirty());
        assert_eq!(emu.battery().unwrap()[0x100], 0xAB);
        assert!(!emu.set_battery(&ram[..1]));
        assert_ne!(emu.rom_crc32(), 0);
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn keys_reach_the_shared_ui_and_hotkeys() {
        let mut emu = Emulator::new(&synthetic_rom(false)).unwrap();
        emu.set_now_ms(1_000_000.0);
        assert!(!emu.ui_active());
        assert!(!emu.overlay_visible());
        assert_eq!(emu.overlay_size(), vec![720, 672]);
        // Controller keys are the page's.
        assert!(!emu.key_down("KeyZ"));
        assert!(!emu.key_down("Enter"));
        // Hotkeys are the library's.
        assert!(emu.key_down("KeyP"));
        assert!(emu.paused());
        assert!(emu.overlay_visible());
        assert!(emu.key_down("KeyN"));
        assert!(emu.take_frame_advance());
        assert!(!emu.take_frame_advance());
        emu.set_paused(false);
        assert!(!emu.paused());
        assert!(emu.key_down("KeyM"));
        assert!(emu.muted());
        emu.toggle_mute();
        assert!(!emu.muted());
        assert!(emu.key_down("Minus"));
        assert!((emu.volume() - 0.9).abs() < 0.001);
        assert!(emu.key_down("Equal"));
        assert!((emu.volume() - 1.0).abs() < 0.001);

        // The palette consumes everything while open and draws with alpha.
        assert!(emu.key_down("Backquote"));
        assert!(emu.ui_active());
        assert!(emu.key_down("KeyZ"));
        assert!(emu.key_down("ShiftLeft"));
        let overlay = emu.overlay_rgba().unwrap();
        assert_eq!(overlay.len(), 720 * 672 * 4);
        assert!(overlay.iter().skip(3).step_by(4).any(|&a| a > 0));
        assert!(emu.key_down("Escape"));
        assert!(!emu.ui_active());
        // Toast expiry follows the clock.
        emu.set_now_ms(1_000_000.0 + 5000.0);
        assert!(!emu.overlay_visible());
        emu.osd_message("hello");
        assert!(emu.overlay_visible());
        assert_eq!(emu.osd_text().as_deref(), Some("hello"));
        let overlay = emu.overlay_rgba().unwrap();
        assert!(overlay.iter().skip(3).step_by(4).any(|&a| a > 0));
        emu.set_now_ms(1_000_000.0 + 8000.0);
        assert!(!emu.overlay_visible());
        assert_eq!(emu.osd_text(), None);
        assert!(emu.overlay_rgba().unwrap().iter().all(|&b| b == 0));
    }

    #[test]
    fn crop_toggle_resizes_the_overlay() {
        let mut emu = Emulator::new(&synthetic_rom(false)).unwrap();
        assert!(emu.crop_enabled());
        emu.set_crop(false);
        emu.tick();
        assert!(!emu.crop_enabled());
        assert_eq!(emu.overlay_size(), vec![768, 720]);
        emu.set_crop(true);
        emu.tick();
        assert_eq!(emu.overlay_size(), vec![720, 672]);
    }

    #[test]
    fn rewind_and_slots_through_the_host() {
        let mut emu = Emulator::new(&synthetic_rom(false)).unwrap();
        emu.set_now_ms(1_700_000_000_000.0);
        for _ in 0..20 {
            emu.run_frame();
        }
        let before = emu.rewind_seconds();
        assert!(before > 0.3);
        assert!(emu.key_down("Backspace"));
        assert!(emu.rewinding());
        emu.rewind_step();
        emu.rewind_step();
        assert!(emu.rewind_seconds() < before);
        emu.key_up("Backspace");
        assert!(!emu.rewinding());

        assert_eq!(emu.take_dirty_slots(), Vec::<u8>::new());
        assert!(emu.key_down("F5"));
        assert_eq!(emu.take_dirty_slots(), vec![1]);
        assert_eq!(emu.take_dirty_slots(), Vec::<u8>::new());
        let bytes = emu.slot_bytes(1).unwrap();
        assert!(bytes.starts_with(b"NESS"));
        assert!(emu.slot_bytes(2).is_none());
        assert!(emu.key_down("F7"));
        assert_eq!(emu.slot(), 2);
        assert!(emu.key_down("F8"));
        assert_eq!(emu.take_dirty_slots(), Vec::<u8>::new());

        emu.set_slot_cache(3, &bytes, 1_700_000_000_000.0);
        assert_eq!(emu.take_dirty_slots(), Vec::<u8>::new());
        emu.load_slot(3);
        assert_eq!(emu.slot(), 3);
        emu.save_slot(4);
        assert_eq!(emu.take_dirty_slots(), vec![4]);
        emu.clear_slot_cache(4);
        assert!(emu.slot_bytes(4).is_none());

        // A cheat added through the palette marks the cheats dirty.
        assert!(!emu.take_cheats_dirty());
        emu.key_down("Backquote");
        for code in [
            "KeyC", "KeyH", "KeyE", "KeyA", "KeyT", "Space", "KeyA", "KeyD", "KeyD", "Space",
            "KeyS", "KeyX", "KeyI", "KeyO", "KeyP", "KeyO", "Enter",
        ] {
            emu.key_down(code);
        }
        assert!(!emu.ui_active());
        assert!(emu.take_cheats_dirty());
        assert!(emu.cheats_text().contains("SXIOPO"));
    }
}
