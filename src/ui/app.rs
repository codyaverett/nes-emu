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
}
