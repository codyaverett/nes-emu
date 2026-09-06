//! In-window UI: overlay text, the command palette and tool pages,
//! shared by the SDL binary and the web page
//! (docs/plans/SHARED_OVERLAY_UI.md, docs/debugging/UI_FRAMEWORK.md).
//!
//! `Ui` is a small state machine. In `Game` mode every key goes to the
//! shared hotkeys ([`Ui::key_down`]) and then the frontend's controller
//! mapping; the backquote key opens the palette. In `Palette` and `Tool`
//! modes the UI consumes every key press and the game pad sees nothing
//! (key releases still reach it so no button sticks). Drawing goes
//! through a [`Painter`]: [`draw_messages`] then [`Ui::draw`] run after
//! the game frame has been presented, so overlays composite over it.

pub mod app;
pub mod commands;
pub mod font;
pub mod host;
pub mod key;
pub mod painter;
pub mod palette;
pub mod tool;
pub mod tools;

use app::App;
use commands::Action;
use key::Key;
use painter::{Color, Painter};
use palette::{Palette, PaletteEvent};
use tool::{Tool, ToolEvent};
use tools::ToolId;

/// Window pixels per NES pixel: the SDL window is the visible picture at
/// this scale, and the web overlay canvas has the same size so the UI
/// draws identically on both.
pub const WINDOW_SCALE: u32 = 3;

/// Font pixels per window pixel. 2 gives 16 px glyphs, 45 columns on
/// the default 720 px wide window; the window scale (3) would leave only
/// 30 columns, too few for a name plus a description.
pub const DEFAULT_FONT_SCALE: u32 = 2;

/// Overlay pixel size for a visible picture of `width` by `height` NES
/// pixels (`App::visible_size`).
pub fn overlay_size(width: u32, height: u32) -> (u32, u32) {
    (width * WINDOW_SCALE, height * WINDOW_SCALE)
}

/// Key bindings outside the palette, as shown on the Help page.
pub const KEY_BINDINGS: &[(&str, &str)] = &[
    ("Backquote", "Open the command palette"),
    ("F1", "Open this help page"),
    ("Escape", "Close overlay; in game, quit"),
    ("P", "Pause / resume"),
    ("N", "Frame advance (pauses first)"),
    ("R", "Reset"),
    ("M", "Mute / unmute"),
    ("Plus / Minus", "Volume up / down"),
    ("F5 / F8", "Save / load state (slot)"),
    ("F6 / F7", "Previous / next state slot"),
    ("Backspace", "Hold to rewind (2x back)"),
    ("Z / X", "A / B"),
    ("Right Shift", "Select"),
    ("Return", "Start"),
    ("Arrows", "D-pad"),
    ("Quote / ;", "P2 A / B"),
    ("Period / ,", "P2 Start / Select"),
    ("I/J/K/L", "P2 D-pad"),
    ("In pages", "Left/Right views, 1-5 toggles"),
];

enum Mode {
    Game,
    Palette(Palette),
    Tool(Box<dyn Tool>),
}

pub struct Ui {
    mode: Mode,
    pub font_scale: u32,
}

impl Ui {
    pub fn new(font_scale: u32) -> Self {
        Ui {
            mode: Mode::Game,
            font_scale,
        }
    }

    pub fn open_palette(&mut self) {
        self.mode = Mode::Palette(Palette::new());
    }

    pub fn open_tool(&mut self, id: ToolId) {
        self.mode = Mode::Tool(id.open());
    }

    pub fn close(&mut self) {
        self.mode = Mode::Game;
    }

    /// True while the palette or a page is open and owns the keyboard.
    pub fn is_active(&self) -> bool {
        !matches!(self.mode, Mode::Game)
    }

    /// A key press from the frontend: the palette or open page first, then
    /// the shared hotkeys. Returns true when the press was used, so the
    /// frontend must not pass it to the controller mapping (or, on the
    /// web, let the browser act on it: F5 must not reload the page).
    /// Escape in `Game` mode is left to the frontend (the SDL binary
    /// quits on it, the web page ignores it), as is F for fullscreen.
    pub fn key_down(&mut self, key: Key, app: &mut App) -> bool {
        if self.handle_key(key, app) {
            return true;
        }
        match key {
            Key::F1 => self.open_tool(ToolId::Help),
            Key::Char('r') => app.reset(),
            Key::Char('p') => app.toggle_pause(),
            Key::Char('n') => app.request_frame_advance(),
            Key::Char('m') => app.toggle_mute(),
            Key::Char('=') | Key::Char('+') => app.volume_up(),
            Key::Char('-') => app.volume_down(),
            // Save states (docs/debugging/SAVE_STATES.md).
            Key::F5 => app.save_state(),
            Key::F6 => app.prev_slot(),
            Key::F7 => app.next_slot(),
            Key::F8 => app.load_state(),
            // Rewind while held (docs/debugging/UI_FRAMEWORK.md, issue
            // #44); `key_up` ends it.
            Key::Backspace => app.rewind_start(),
            _ => return false,
        }
        true
    }

    /// A key release. Releases are never consumed: the frontend passes
    /// every one to the controller so a button held when the palette
    /// opened does not stick.
    pub fn key_up(&mut self, key: Key, app: &mut App) {
        if key == Key::Backspace {
            // A no-op when the press went to the palette or a page.
            app.rewind_stop();
        }
    }

    /// Route a key press to the palette or the open page only. Returns
    /// true when the UI consumed it.
    pub fn handle_key(&mut self, key: Key, app: &mut App) -> bool {
        enum Outcome {
            NotConsumed,
            Consumed,
            OpenPalette,
            Close,
            Run(usize, String),
        }
        let outcome = match &mut self.mode {
            Mode::Game => {
                if key == Key::Backquote {
                    Outcome::OpenPalette
                } else {
                    Outcome::NotConsumed
                }
            }
            Mode::Palette(palette) => match palette.handle_key(key, &app.commands) {
                PaletteEvent::Continue => Outcome::Consumed,
                PaletteEvent::Close => Outcome::Close,
                PaletteEvent::Run(index, arg) => Outcome::Run(index, arg),
            },
            Mode::Tool(tool) => {
                if key == Key::Escape && !tool.captures_escape() {
                    Outcome::Close
                } else {
                    match tool.handle_key(key, app) {
                        ToolEvent::Continue => Outcome::Consumed,
                        ToolEvent::Close => Outcome::Close,
                    }
                }
            }
        };
        match outcome {
            Outcome::NotConsumed => false,
            Outcome::Consumed => true,
            Outcome::OpenPalette => {
                self.open_palette();
                true
            }
            Outcome::Close => {
                self.close();
                true
            }
            Outcome::Run(index, arg) => {
                self.close();
                self.run_command(index, &arg, app);
                true
            }
        }
    }

    /// Run the command at `index` in `app.commands` with `arg` (empty for
    /// commands without one). A command that sets `App::pending_tool`
    /// gets that page opened afterwards.
    pub fn run_command(&mut self, index: usize, arg: &str, app: &mut App) {
        let name = app.commands[index].name;
        let action = app.commands[index].action;
        if arg.is_empty() {
            log::info!("Command: {}", name);
        } else {
            log::info!("Command: {} {}", name, arg);
        }
        match action {
            Action::Run(f) => f(app),
            Action::RunWithArg(f) => f(app, arg),
            Action::OpenTool(id) => self.open_tool(id),
        }
        if let Some(id) = app.pending_tool.take() {
            self.open_tool(id);
        }
    }

    /// Per-frame hook for the open tool.
    pub fn tick(&mut self, app: &mut App) {
        if let Mode::Tool(tool) = &mut self.mode {
            tool.tick(app);
        }
    }

    /// Draw the overlay for the current mode over the game frame.
    pub fn draw(&self, painter: &mut dyn Painter, app: &App) -> Result<(), String> {
        match &self.mode {
            Mode::Game => Ok(()),
            Mode::Palette(palette) => palette.draw(painter, self.font_scale, &app.commands),
            Mode::Tool(tool) => {
                tool::draw_frame(painter, self.font_scale, tool.title())?;
                tool.draw(painter, self.font_scale, app)
            }
        }
    }
}

/// Reminder drawn while paused with no toast showing.
pub const PAUSED_MESSAGE: &str = "PAUSED   P resume   N step   Bksp rewind";

/// The volume/mute indicator: a bar near the top-left corner, red while
/// muted, green to the current volume otherwise.
fn draw_osd(painter: &mut dyn Painter, app: &App) -> Result<(), String> {
    let scale = WINDOW_SCALE;
    let osd_x = 10 * scale as i32;
    let osd_y = 10 * scale as i32;
    let osd_width = 200 * scale;
    let osd_height = 20 * scale;

    painter.fill_rect(
        osd_x,
        osd_y,
        osd_width,
        osd_height,
        Color::rgba(0, 0, 0, 180),
    )?;
    if app.is_muted() {
        painter.fill_rect(
            osd_x + 2 * scale as i32,
            osd_y + 2 * scale as i32,
            osd_width - 4 * scale,
            osd_height - 4 * scale,
            Color::rgb(255, 0, 0),
        )
    } else {
        let filled_width = ((osd_width - 4 * scale) as f32 * app.volume()) as u32;
        painter.fill_rect(
            osd_x + 2 * scale as i32,
            osd_y + 2 * scale as i32,
            filled_width,
            osd_height - 4 * scale,
            Color::rgb(0, 255, 0),
        )
    }
}

/// One line of text over the bottom of the picture ("Saved slot 3").
pub fn draw_message(painter: &mut dyn Painter, font_scale: u32, text: &str) -> Result<(), String> {
    let (w, h) = painter.size();
    let pad = 4 * font_scale as i32;
    let line = font::line_height(font_scale) as i32;
    let y = h as i32 - line - 3 * pad;
    painter.fill_rect(
        0,
        y - pad,
        w,
        (line + 2 * pad) as u32,
        Color::rgba(0, 0, 0, 180),
    )?;
    font::draw_text(painter, pad, y, font_scale, Color::rgb(255, 255, 255), text)
}

/// The volume/mute indicator (only while audio is on and it was just
/// changed), then the current toast, or the paused reminder while paused
/// with no toast. Drawn before [`Ui::draw`] so a page covers them. Uses
/// `app.now_ms` for expiry; see [`App::messages_visible`].
pub fn draw_messages(painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
    if app.osd_bar_visible() {
        draw_osd(painter, app)?;
    }
    if let Some(text) = app.osd_message() {
        draw_message(painter, font_scale, text)
    } else if app.paused && !app.rewinding {
        draw_message(painter, font_scale, PAUSED_MESSAGE)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::System;
    use host::{Host, SlotInfo};
    use painter::RgbaPainter;
    use std::sync::{Arc, Mutex};

    struct NoHost;

    impl Host for NoHost {
        fn write_state(&mut self, _: u8, _: &[u8]) -> Result<(), String> {
            Err("no".into())
        }
        fn read_state(&mut self, _: u8) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn slot_info(&self, _: u8) -> Option<SlotInfo> {
            None
        }
        fn slot_label(&self, slot: u8) -> String {
            format!("slot {slot}")
        }
        fn write_cheats(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn cheats_label(&self) -> String {
            "none".into()
        }
    }

    fn app() -> App {
        App::new(
            System::new(),
            None,
            Arc::new(Mutex::new(false)),
            Arc::new(Mutex::new(0.5)),
            true,
            Box::new(NoHost),
        )
    }

    #[test]
    fn game_mode_runs_hotkeys_and_leaves_the_rest() {
        let mut app = app();
        let mut ui = Ui::new(DEFAULT_FONT_SCALE);
        assert!(!ui.is_active());
        assert!(ui.key_down(Key::Char('p'), &mut app));
        assert!(app.paused);
        assert!(ui.key_down(Key::F7, &mut app));
        assert_eq!(app.slot, 2);
        assert!(ui.key_down(Key::Backspace, &mut app));
        assert!(app.rewinding);
        ui.key_up(Key::Backspace, &mut app);
        assert!(!app.rewinding);
        // Controller keys, Escape and F are the frontend's.
        assert!(!ui.key_down(Key::Char('z'), &mut app));
        assert!(!ui.key_down(Key::Escape, &mut app));
        assert!(!ui.key_down(Key::Char('f'), &mut app));
        assert!(!ui.key_down(Key::Return, &mut app));
        assert!(ui.key_down(Key::F1, &mut app));
        assert!(ui.is_active());
    }

    #[test]
    fn open_ui_consumes_every_key() {
        let mut app = app();
        let mut ui = Ui::new(DEFAULT_FONT_SCALE);
        assert!(ui.key_down(Key::Backquote, &mut app));
        assert!(ui.is_active());
        // Typing into the palette must not reach the hotkeys or the pad.
        assert!(ui.key_down(Key::Char('p'), &mut app));
        assert!(!app.paused);
        assert!(ui.key_down(Key::Char('z'), &mut app));
        assert!(ui.key_down(Key::Other, &mut app));
        assert!(ui.key_down(Key::Escape, &mut app));
        assert!(!ui.is_active());
    }

    #[test]
    fn messages_draw_into_an_rgba_overlay() {
        let mut app = app();
        let mut painter = RgbaPainter::new(720, 672);
        draw_messages(&mut painter, DEFAULT_FONT_SCALE, &app).unwrap();
        assert!(painter.pixels().iter().all(|&b| b == 0));
        app.now_ms = 10;
        app.show_message("Saved slot 1");
        draw_messages(&mut painter, DEFAULT_FONT_SCALE, &app).unwrap();
        // The toast band near the bottom has the translucent backdrop.
        let band_y = 672 - 16 - 3 * 8;
        assert_eq!(painter.pixel(700, band_y as u32)[3], 180);
        assert_eq!(painter.pixel(700, 10)[3], 0);
    }
}
