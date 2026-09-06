//! Mutable emulator-facing state shared by the main loop, palette commands
//! and tool pages.
//!
//! `App` owns the `System` and the handles the audio callback reads, plus
//! the run-control flags (paused, frame advance, overscan crop, quit).
//! Commands and tools mutate it; the main loop reads the flags once per
//! frame and applies anything that touches SDL objects (window size, the
//! source rectangle) itself, because those live outside this struct.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sdl2::rect::Rect;

use nes_emu::cheat::{Cheat, CheatError};
use nes_emu::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use nes_emu::system::System;

use super::commands::{self, Command};
use super::tools::ToolId;

/// Pixels hidden on every edge of the picture when overscan cropping is
/// on. A CRT never shows them, and games rely on that: Final Fantasy draws
/// the map row scrolling in at the top one frame before its attribute
/// bytes, so the top eight lines flash the wrong palette on every row
/// change; Super Mario Bros. 3 hides the left eight pixels with the PPU
/// mask and lets the rightmost few pixels show the not-yet-redrawn column
/// while scrolling.
pub const OVERSCAN_PIXELS: u32 = 8;

/// How long the volume/mute indicator stays on screen after a change.
const OSD_DURATION: Duration = Duration::from_secs(2);

/// Samples queued for the SDL audio callback.
pub type AudioQueue = Arc<Mutex<VecDeque<f32>>>;

/// Rewind (docs/debugging/UI_FRAMEWORK.md, issue #44): a snapshot of the
/// machine is taken every `REWIND_INTERVAL_FRAMES` emulated frames while
/// recording is on. Holding Backspace loads them newest first, one per
/// presented frame, so the game runs backwards at
/// `REWIND_INTERVAL_FRAMES` times its normal speed.
pub const REWIND_INTERVAL_FRAMES: u32 = 2;
/// Snapshots kept: 600 at two frames each is 20 seconds at 60 Hz, about
/// 9 MB at 15 KB per image.
pub const REWIND_MAX_ENTRIES: usize = 600;
/// NES frames per second, used to turn snapshot counts into seconds.
const FRAMES_PER_SECOND: f32 = 60.0;

pub struct App {
    pub system: System,
    /// `None` when audio is disabled; then the frame loop paces itself.
    pub audio_buffer: Option<AudioQueue>,
    pub muted: Arc<Mutex<bool>>,
    pub volume: Arc<Mutex<f32>>,
    /// Emulation stops while set; the last frame keeps being presented.
    pub paused: bool,
    /// Run exactly one frame on the next loop iteration while paused.
    pub frame_advance: bool,
    /// Hide `OVERSCAN_PIXELS` on every edge.
    pub crop_enabled: bool,
    /// Set by `toggle_crop`; the main loop clears it after resizing the
    /// window and recreating its source rectangle.
    pub crop_dirty: bool,
    /// Set by the quit command or Escape; the main loop exits, flushing
    /// battery RAM on the way out.
    pub quit_requested: bool,
    /// While `Some` and in the future, the volume/mute indicator is drawn.
    pub osd_until: Option<Instant>,
    /// Battery-backed PRG RAM file next to the ROM.
    pub save_path: PathBuf,
    /// Cheat list next to the ROM (`<rom>.cht`), rewritten on every
    /// change through the `*_cheat*` methods below.
    pub cheat_path: PathBuf,
    /// Last cheat error, shown in red on the Cheats page until the next
    /// successful change. Set by the page and by `cheat add` / `cheat
    /// toggle` from the palette, whose failures would otherwise only
    /// reach the log.
    pub cheat_error: Option<String>,
    /// Every command the palette can run.
    pub commands: Vec<Command>,
    /// Set by a command that wants a page opened once it has run;
    /// `Ui::run_command` takes it.
    pub pending_tool: Option<ToolId>,
    /// First address the memory page shows, kept across opens.
    pub memory_addr: u16,
    /// The ROM file; save states live next to it as `<rom>.s1` .. `.s9`
    /// (docs/debugging/SAVE_STATES.md, issue #39).
    pub rom_path: PathBuf,
    /// Current save-state slot, 1 to [`STATE_SLOTS`].
    pub slot: u8,
    /// One-line message drawn over the game until the instant passes
    /// ("Saved slot 3"). Independent of the volume indicator.
    pub osd_text: Option<(String, Instant)>,
    /// State images, oldest first, taken every `REWIND_INTERVAL_FRAMES`
    /// while `rewind_recording` is on; capped at `REWIND_MAX_ENTRIES`.
    pub rewind_buffer: VecDeque<Vec<u8>>,
    /// Snapshots are taken while set; `rewind off` clears the buffer and
    /// stops recording to save memory.
    pub rewind_recording: bool,
    /// Backspace is held: the main loop pops and loads one snapshot per
    /// presented frame instead of emulating.
    pub rewinding: bool,
    /// Emulated frames since the last snapshot.
    rewind_frame_counter: u32,
    /// Set once the first snapshot has been timed and logged.
    rewind_cost_logged: bool,
}

/// Number of save-state slots.
pub const STATE_SLOTS: u8 = 9;

impl App {
    pub fn new(
        system: System,
        audio_buffer: Option<AudioQueue>,
        muted: Arc<Mutex<bool>>,
        volume: Arc<Mutex<f32>>,
        crop_enabled: bool,
        rom_path: PathBuf,
    ) -> Self {
        App {
            system,
            audio_buffer,
            muted,
            volume,
            paused: false,
            frame_advance: false,
            crop_enabled,
            crop_dirty: false,
            quit_requested: false,
            osd_until: None,
            save_path: rom_path.with_extension("sav"),
            cheat_path: rom_path.with_extension("cht"),
            cheat_error: None,
            commands: commands::builtin_commands(),
            pending_tool: None,
            memory_addr: 0,
            rom_path,
            slot: 1,
            osd_text: None,
            rewind_buffer: VecDeque::with_capacity(REWIND_MAX_ENTRIES),
            rewind_recording: true,
            rewinding: false,
            rewind_frame_counter: 0,
            rewind_cost_logged: false,
        }
    }

    pub fn audio_enabled(&self) -> bool {
        self.audio_buffer.is_some()
    }

    // Save states (docs/debugging/SAVE_STATES.md, issue #39).

    /// Show `text` over the game for `OSD_DURATION`.
    pub fn show_message(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::info!("{}", text);
        self.osd_text = Some((text, Instant::now() + OSD_DURATION));
    }

    /// The message to draw this frame, if one is still current.
    pub fn osd_message(&self) -> Option<&str> {
        match &self.osd_text {
            Some((text, until)) if Instant::now() < *until => Some(text),
            _ => None,
        }
    }

    /// `<rom>.sN` for slot `n`.
    pub fn state_path(&self, slot: u8) -> PathBuf {
        self.rom_path.with_extension(format!("s{slot}"))
    }

    /// Snapshot the machine into `slot` and make it the current slot.
    pub fn save_state_to(&mut self, slot: u8) {
        if !(1..=STATE_SLOTS).contains(&slot) {
            self.show_message(format!("Slot must be 1-{STATE_SLOTS}"));
            return;
        }
        self.slot = slot;
        let path = self.state_path(slot);
        let image = self.system.save_state();
        match std::fs::write(&path, &image) {
            Ok(()) => self.show_message(format!("Saved slot {slot}")),
            Err(e) => {
                log::warn!("Could not write {}: {}", path.display(), e);
                self.show_message(format!("Save failed: {e}"));
            }
        }
    }

    /// Restore the machine from `slot` and make it the current slot. A
    /// missing file or a state for another ROM is reported on the OSD
    /// and leaves the machine untouched. While paused, one frame is run
    /// so the picture shows the restored state.
    pub fn load_state_from(&mut self, slot: u8) {
        if !(1..=STATE_SLOTS).contains(&slot) {
            self.show_message(format!("Slot must be 1-{STATE_SLOTS}"));
            return;
        }
        self.slot = slot;
        let path = self.state_path(slot);
        let image = match std::fs::read(&path) {
            Ok(image) => image,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.show_message(format!("Slot {slot} is empty"));
                return;
            }
            Err(e) => {
                log::warn!("Could not read {}: {}", path.display(), e);
                self.show_message(format!("Load failed: {e}"));
                return;
            }
        };
        match self.system.load_state(&image) {
            Ok(()) => {
                if self.paused {
                    self.frame_advance = true;
                }
                self.show_message(format!("Loaded slot {slot}"));
            }
            Err(e) => {
                log::warn!("Could not load {}: {}", path.display(), e);
                self.show_message(format!("Load failed: {e}"));
            }
        }
    }

    /// F5: save to the current slot.
    pub fn save_state(&mut self) {
        self.save_state_to(self.slot);
    }

    /// F8: load the current slot.
    pub fn load_state(&mut self) {
        self.load_state_from(self.slot);
    }

    /// F6 / F7: step the current slot, wrapping.
    pub fn prev_slot(&mut self) {
        self.set_slot(if self.slot <= 1 {
            STATE_SLOTS
        } else {
            self.slot - 1
        });
    }

    pub fn next_slot(&mut self) {
        self.set_slot(if self.slot >= STATE_SLOTS {
            1
        } else {
            self.slot + 1
        });
    }

    pub fn set_slot(&mut self, slot: u8) {
        if !(1..=STATE_SLOTS).contains(&slot) {
            self.show_message(format!("Slot must be 1-{STATE_SLOTS}"));
            return;
        }
        self.slot = slot;
        let status = if self.state_path(slot).exists() {
            "saved"
        } else {
            "empty"
        };
        self.show_message(format!("Slot {slot} ({status})"));
    }

    /// Slot number from a palette argument: empty means the current slot.
    fn slot_argument(&self, arg: &str) -> Option<u8> {
        let arg = arg.trim();
        if arg.is_empty() {
            return Some(self.slot);
        }
        arg.parse::<u8>()
            .ok()
            .filter(|n| (1..=STATE_SLOTS).contains(n))
    }

    /// `save state [N]` from the palette.
    pub fn save_state_command(&mut self, arg: &str) {
        match self.slot_argument(arg) {
            Some(slot) => self.save_state_to(slot),
            None => self.show_message(format!("save state needs a slot 1-{STATE_SLOTS}")),
        }
    }

    /// `load state [N]` from the palette.
    pub fn load_state_command(&mut self, arg: &str) {
        match self.slot_argument(arg) {
            Some(slot) => self.load_state_from(slot),
            None => self.show_message(format!("load state needs a slot 1-{STATE_SLOTS}")),
        }
    }

    /// `slot N` from the palette; a bare `slot` opens the States page.
    pub fn slot_command(&mut self, arg: &str) {
        if arg.trim().is_empty() {
            self.pending_tool = Some(ToolId::States);
            return;
        }
        match self.slot_argument(arg) {
            Some(slot) => self.set_slot(slot),
            None => self.show_message(format!("slot needs a number 1-{STATE_SLOTS}")),
        }
    }

    // Rewind (docs/debugging/UI_FRAMEWORK.md, issue #44).

    /// Call once per emulated frame (not for the frame `rewind_step` runs
    /// to refresh the picture): every `REWIND_INTERVAL_FRAMES` a snapshot
    /// is pushed and the oldest dropped past `REWIND_MAX_ENTRIES`. The
    /// first snapshot's cost is logged at info level.
    pub fn record_rewind_frame(&mut self) {
        if !self.rewind_recording {
            return;
        }
        self.rewind_frame_counter += 1;
        if self.rewind_frame_counter < REWIND_INTERVAL_FRAMES {
            return;
        }
        self.rewind_frame_counter = 0;
        let started = Instant::now();
        let image = self.system.save_state();
        if !self.rewind_cost_logged {
            self.rewind_cost_logged = true;
            log::info!(
                "Rewind snapshot: {} bytes in {:?} (every {} frames, {} kept, {:.1} MB max)",
                image.len(),
                started.elapsed(),
                REWIND_INTERVAL_FRAMES,
                REWIND_MAX_ENTRIES,
                (image.len() * REWIND_MAX_ENTRIES) as f32 / 1e6
            );
        }
        if self.rewind_buffer.len() >= REWIND_MAX_ENTRIES {
            self.rewind_buffer.pop_front();
        }
        self.rewind_buffer.push_back(image);
    }

    /// Seconds of gameplay the buffer can rewind through.
    pub fn rewind_seconds(&self) -> f32 {
        (self.rewind_buffer.len() as u32 * REWIND_INTERVAL_FRAMES) as f32 / FRAMES_PER_SECOND
    }

    /// Backspace pressed in game mode. Idempotent: SDL repeats key-down
    /// events while a key is held.
    pub fn rewind_start(&mut self) {
        if !self.rewinding {
            log::info!("Rewinding ({:.1} s available)", self.rewind_seconds());
        }
        self.rewinding = true;
    }

    /// Backspace released: emulation resumes from the last loaded
    /// snapshot; the buffer keeps only what is older than that point.
    /// Harmless when no rewind was in progress (the palette ate the
    /// press). The interval counter restarts so the next snapshot is a
    /// full interval after the resume point.
    pub fn rewind_stop(&mut self) {
        if self.rewinding {
            log::info!("Rewind stopped ({:.1} s left)", self.rewind_seconds());
            self.rewind_frame_counter = 0;
        }
        self.rewinding = false;
    }

    /// Pop the newest snapshot and load it, then run one frame without
    /// audio so the picture shows it (the frame buffer is not part of
    /// the image). Returns false with the machine untouched when the
    /// buffer is empty. Called by the main loop once per presented frame
    /// while `rewinding`; the caller drains the audio queue and skips
    /// normal emulation.
    pub fn rewind_step(&mut self) -> bool {
        let Some(image) = self.rewind_buffer.pop_back() else {
            return false;
        };
        if let Err(e) = self.system.load_state(&image) {
            // Every image came from this machine, so this cannot happen
            // short of a bug; drop the buffer rather than loop on it.
            log::warn!("Rewind snapshot rejected: {}; clearing buffer", e);
            self.rewind_buffer.clear();
            return false;
        }
        self.system.run_frame();
        true
    }

    /// Jump back `seconds` at once: pops the snapshots covering that
    /// span (or all of them) and loads the last one.
    pub fn rewind_by_seconds(&mut self, seconds: f32) {
        let per_snapshot = REWIND_INTERVAL_FRAMES as f32 / FRAMES_PER_SECOND;
        let wanted = (seconds / per_snapshot).ceil().max(1.0) as usize;
        let available = self.rewind_buffer.len();
        if available == 0 {
            self.show_message("Nothing to rewind");
            return;
        }
        let count = wanted.min(available);
        // Drop the intermediate images; the last one is loaded.
        for _ in 1..count {
            self.rewind_buffer.pop_back();
        }
        if self.rewind_step() {
            self.rewind_frame_counter = 0;
            let went = count as f32 * per_snapshot;
            let left = self.rewind_seconds();
            self.show_message(format!("Rewound {went:.1} s ({left:.1} s left)"));
        }
    }

    /// Turn recording on or off; off clears the buffer to free memory.
    pub fn set_rewind_recording(&mut self, on: bool) {
        self.rewind_recording = on;
        self.rewind_frame_counter = 0;
        if on {
            self.show_message("Rewind recording on");
        } else {
            self.rewind_buffer.clear();
            self.show_message("Rewind recording off (buffer cleared)");
        }
    }

    /// One line for the Help page and the bare `rewind` command. At
    /// most 43 characters so it fits the 44-column page.
    pub fn rewind_status(&self) -> String {
        if self.rewind_recording {
            format!(
                "Rewind: on, {:.1} s buffered ({} snapshots)",
                self.rewind_seconds(),
                self.rewind_buffer.len()
            )
        } else {
            "Rewind: off (rewind on to record)".to_string()
        }
    }

    /// `rewind N` (seconds back), `rewind on`, `rewind off`; a bare
    /// `rewind` shows the recording state.
    pub fn rewind_command(&mut self, arg: &str) {
        let arg = arg.trim();
        match arg.to_ascii_lowercase().as_str() {
            "" => {
                let status = self.rewind_status();
                self.show_message(status);
            }
            "on" => self.set_rewind_recording(true),
            "off" => self.set_rewind_recording(false),
            _ => match arg.parse::<f32>() {
                Ok(seconds) if seconds > 0.0 && seconds.is_finite() => {
                    self.rewind_by_seconds(seconds)
                }
                _ => self.show_message("rewind needs seconds, on or off"),
            },
        }
    }

    /// Pixels cropped from each edge of the 256x240 frame.
    pub fn crop(&self) -> u32 {
        if self.crop_enabled {
            OVERSCAN_PIXELS
        } else {
            0
        }
    }

    /// Visible picture size in NES pixels after cropping.
    pub fn visible_size(&self) -> (u32, u32) {
        let crop = self.crop();
        (
            SCREEN_WIDTH as u32 - 2 * crop,
            SCREEN_HEIGHT as u32 - 2 * crop,
        )
    }

    /// The part of the frame texture that is copied to the window.
    pub fn src_rect(&self) -> Rect {
        let crop = self.crop();
        let (w, h) = self.visible_size();
        Rect::new(crop as i32, crop as i32, w, h)
    }

    pub fn is_muted(&self) -> bool {
        *self.muted.lock().unwrap()
    }

    pub fn volume(&self) -> f32 {
        *self.volume.lock().unwrap()
    }

    fn show_osd(&mut self) {
        self.osd_until = Some(Instant::now() + OSD_DURATION);
    }

    pub fn pause(&mut self) {
        if !self.paused {
            self.show_message("Paused  (P resumes, N steps one frame)");
        }
        self.paused = true;
        self.frame_advance = false;
    }

    pub fn resume(&mut self) {
        if self.paused {
            self.show_message("Resumed");
        }
        self.paused = false;
        self.frame_advance = false;
    }

    pub fn toggle_pause(&mut self) {
        if self.paused {
            self.resume();
        } else {
            self.pause();
        }
    }

    /// Pause if running, and run one frame on the next loop iteration.
    pub fn request_frame_advance(&mut self) {
        self.paused = true;
        self.frame_advance = true;
        self.show_message("Frame advance  (paused; P resumes)");
    }

    pub fn reset(&mut self) {
        self.show_message("Reset");
        self.system.reset();
    }

    pub fn toggle_mute(&mut self) {
        let now = {
            let mut m = self.muted.lock().unwrap();
            *m = !*m;
            *m
        };
        self.show_message(if now { "Muted" } else { "Unmuted" });
        self.show_osd();
    }

    pub fn volume_up(&mut self) {
        self.adjust_volume(0.1);
    }

    pub fn volume_down(&mut self) {
        self.adjust_volume(-0.1);
    }

    fn adjust_volume(&mut self, delta: f32) {
        let now = {
            let mut v = self.volume.lock().unwrap();
            *v = (*v + delta).clamp(0.0, 1.0);
            *v
        };
        self.show_message(format!("Volume {:.0}%", now * 100.0));
        self.show_osd();
    }

    pub fn toggle_crop(&mut self) {
        self.crop_enabled = !self.crop_enabled;
        self.crop_dirty = true;
        self.show_message(if self.crop_enabled {
            "Overscan crop on (8 px hidden each edge)"
        } else {
            "Overscan crop off (full 256x240)"
        });
    }

    pub fn quit(&mut self) {
        self.quit_requested = true;
    }

    // Cheats (docs/debugging/CHEAT_ENGINE.md, issue #32). Every mutation
    // goes through one of these so the .cht file is rewritten on change.

    /// Rewrite `cheat_path` from the current set; failures are logged.
    fn save_cheats(&mut self) {
        if let Err(e) = self.system.save_cheats(&self.cheat_path) {
            let message = format!("Could not write {}: {}", self.cheat_path.display(), e);
            log::warn!("{}", message);
            self.cheat_error = Some(message);
        }
    }

    /// Parse `code` and append it with `description`, enabled. Returns
    /// the new index; on failure the error is also kept in `cheat_error`.
    ///
    /// Key codes cannot type a shifted character (`:` arrives as `;` and
    /// `?` as `/`), so those two are mapped before parsing; neither is
    /// valid in any code.
    pub fn add_cheat(&mut self, code: &str, description: &str) -> Result<usize, CheatError> {
        let code = code.replace(';', ":").replace('/', "?");
        match Cheat::parse(&code) {
            Ok(cheat) => {
                let index = self
                    .system
                    .cheats_mut()
                    .add(cheat.with_description(description.trim()));
                log::info!("Added cheat {} {}", code, description.trim());
                self.cheat_error = None;
                self.save_cheats();
                Ok(index)
            }
            Err(e) => {
                log::warn!("Cheat {:?} rejected: {}", code, e);
                self.cheat_error = Some(format!("{}: {}", code.trim(), e));
                Err(e)
            }
        }
    }

    /// Flip the enabled flag of cheat `index`; returns the new state.
    pub fn toggle_cheat(&mut self, index: usize) -> Option<bool> {
        let enabled = self.system.cheats_mut().toggle(index)?;
        let code = self.system.cheats().get(index).map(|c| c.code.clone());
        self.show_message(format!(
            "Cheat {} {}",
            code.unwrap_or_default(),
            if enabled { "enabled" } else { "disabled" }
        ));
        self.cheat_error = None;
        self.save_cheats();
        Some(enabled)
    }

    pub fn remove_cheat(&mut self, index: usize) -> Option<Cheat> {
        let cheat = self.system.cheats_mut().remove(index)?;
        log::info!("Removed cheat {}", cheat.code);
        self.cheat_error = None;
        self.save_cheats();
        Some(cheat)
    }

    pub fn set_cheat_description(&mut self, index: usize, description: &str) -> Option<()> {
        self.system
            .cheats_mut()
            .set_description(index, description.trim())?;
        self.cheat_error = None;
        self.save_cheats();
        Some(())
    }

    /// `cheat clear`: delete every cheat.
    pub fn clear_cheats(&mut self) {
        let n = self.system.cheats().len();
        self.system.cheats_mut().clear();
        log::info!("Cleared {} cheat(s)", n);
        self.cheat_error = None;
        self.save_cheats();
    }

    /// `cheat add CODE [description]` from the palette.
    pub fn cheat_add_command(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            log::warn!("cheat add needs a code, e.g. cheat add SXIOPO");
            self.cheat_error = Some("cheat add needs a code".into());
            return;
        }
        let (code, description) = match arg.split_once(' ') {
            Some((code, rest)) => (code, rest),
            None => (arg, ""),
        };
        // The result is already logged and recorded in `cheat_error`.
        let _ = self.add_cheat(code, description);
    }

    /// `cheat toggle N` from the palette; N is 1-based as shown on the page.
    pub fn cheat_toggle_command(&mut self, arg: &str) {
        let count = self.system.cheats().len();
        match arg.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= count => {
                self.toggle_cheat(n - 1);
            }
            _ => {
                let message = if count == 0 {
                    "cheat toggle: no cheats loaded".to_string()
                } else {
                    format!("cheat toggle needs a number 1-{}", count)
                };
                log::warn!("{} (got {:?})", message, arg);
                self.cheat_error = Some(message);
            }
        }
    }
    // ---- Example tools (issue 33) ----

    /// `mem [ADDR]`: open the memory page, at hex `ADDR` when given
    /// (`300`, `0x300` and `$300` all work; the row is aligned to 16).
    /// A bad argument keeps the current address and logs a warning.
    pub fn goto_memory(&mut self, arg: &str) {
        if !arg.is_empty() {
            match parse_hex_address(arg) {
                Some(addr) => self.memory_addr = addr & 0xFFF0,
                None => log::warn!("mem: not a hex address: {:?}", arg),
            }
        }
        self.pending_tool = Some(ToolId::Memory);
    }

    pub fn mute_channel(&mut self, ch: usize) {
        self.system.apu.set_channel_muted(ch, true);
        self.show_message(format!("{} muted", nes_emu::apu::CHANNEL_NAMES[ch]));
    }

    pub fn toggle_channel_mute(&mut self, ch: usize) {
        let muted = self.system.apu.channel_muted(ch);
        self.system.apu.set_channel_muted(ch, !muted);
        self.show_message(format!(
            "{} {}",
            nes_emu::apu::CHANNEL_NAMES[ch],
            if muted { "unmuted" } else { "muted" }
        ));
    }

    pub fn unmute_all(&mut self) {
        for ch in 0..nes_emu::apu::CHANNEL_COUNT {
            self.system.apu.set_channel_muted(ch, false);
        }
        self.show_message("All channels unmuted");
    }
}

/// Parse a 16-bit hex address with an optional `0x` or `$` prefix.
pub fn parse_hex_address(text: &str) -> Option<u16> {
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .or_else(|| text.strip_prefix('$'))
        .unwrap_or(text);
    u16::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_hex_address;

    #[test]
    fn hex_addresses_accept_common_prefixes() {
        assert_eq!(parse_hex_address("300"), Some(0x300));
        assert_eq!(parse_hex_address(" 0xC000 "), Some(0xC000));
        assert_eq!(parse_hex_address("$ff"), Some(0xFF));
        assert_eq!(parse_hex_address("10000"), None);
        assert_eq!(parse_hex_address("zz"), None);
        assert_eq!(parse_hex_address(""), None);
    }
}
