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
}

impl App {
    pub fn new(
        system: System,
        audio_buffer: Option<AudioQueue>,
        muted: Arc<Mutex<bool>>,
        volume: Arc<Mutex<f32>>,
        crop_enabled: bool,
        save_path: PathBuf,
        cheat_path: PathBuf,
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
            save_path,
            cheat_path,
            cheat_error: None,
            commands: commands::builtin_commands(),
        }
    }

    pub fn audio_enabled(&self) -> bool {
        self.audio_buffer.is_some()
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
            log::info!("Paused");
        }
        self.paused = true;
        self.frame_advance = false;
    }

    pub fn resume(&mut self) {
        if self.paused {
            log::info!("Resumed");
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
    }

    pub fn reset(&mut self) {
        log::info!("Resetting NES...");
        self.system.reset();
    }

    pub fn toggle_mute(&mut self) {
        let now = {
            let mut m = self.muted.lock().unwrap();
            *m = !*m;
            *m
        };
        log::info!("Audio {}", if now { "muted" } else { "unmuted" });
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
        log::info!("Volume: {:.0}%", now * 100.0);
        self.show_osd();
    }

    pub fn toggle_crop(&mut self) {
        self.crop_enabled = !self.crop_enabled;
        self.crop_dirty = true;
        log::info!(
            "Overscan crop {}",
            if self.crop_enabled { "on" } else { "off" }
        );
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
        log::info!(
            "Cheat {} {}",
            code.unwrap_or_default(),
            if enabled { "enabled" } else { "disabled" }
        );
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
}
