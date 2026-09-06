//! In-window UI: overlay text, the command palette and tool pages.
//!
//! `Ui` is a small state machine. In `Game` mode every key goes to the
//! emulator's hotkeys and controller mapping; the backquote key opens the
//! palette. In `Palette` and `Tool` modes the UI consumes every key press
//! and the game pad sees nothing (key releases still reach it so no button
//! sticks). `Ui::draw` runs after the game frame has been copied to the
//! canvas, so overlays composite over the picture.

pub mod app;
pub mod commands;
pub mod font;
pub mod palette;
pub mod tool;
pub mod tools;

use sdl2::keyboard::Keycode;
use sdl2::render::WindowCanvas;

use app::App;
use commands::Action;
use palette::{Palette, PaletteEvent};
use tool::{Tool, ToolEvent};
use tools::ToolId;

/// Font pixels per window pixel. 2 gives 16 px glyphs, 45 columns on
/// the default 720 px wide window; the window scale (3) would leave only
/// 30 columns, too few for a name plus a description.
pub const DEFAULT_FONT_SCALE: u32 = 2;

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
    ("Z / X", "A / B"),
    ("Right Shift", "Select"),
    ("Return", "Start"),
    ("Arrows", "D-pad"),
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

    /// Route a key press. Returns true when the UI consumed it.
    pub fn handle_key(&mut self, key: Keycode, app: &mut App) -> bool {
        enum Outcome {
            NotConsumed,
            Consumed,
            OpenPalette,
            Close,
            Run(usize),
        }
        let outcome = match &mut self.mode {
            Mode::Game => {
                if key == Keycode::Backquote {
                    Outcome::OpenPalette
                } else {
                    Outcome::NotConsumed
                }
            }
            Mode::Palette(palette) => match palette.handle_key(key, &app.commands) {
                PaletteEvent::Continue => Outcome::Consumed,
                PaletteEvent::Close => Outcome::Close,
                PaletteEvent::Run(index) => Outcome::Run(index),
            },
            Mode::Tool(tool) => {
                if key == Keycode::Escape {
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
            Outcome::Run(index) => {
                self.close();
                self.run_command(index, app);
                true
            }
        }
    }

    /// Run the command at `index` in `app.commands`.
    pub fn run_command(&mut self, index: usize, app: &mut App) {
        let name = app.commands[index].name;
        let action = app.commands[index].action;
        log::info!("Command: {}", name);
        match action {
            Action::Run(f) => f(app),
            Action::OpenTool(id) => self.open_tool(id),
        }
    }

    /// Per-frame hook for the open tool.
    pub fn tick(&mut self, app: &mut App) {
        if let Mode::Tool(tool) = &mut self.mode {
            tool.tick(app);
        }
    }

    /// Draw the overlay for the current mode over the game frame.
    pub fn draw(&self, canvas: &mut WindowCanvas, app: &App) -> Result<(), String> {
        match &self.mode {
            Mode::Game => Ok(()),
            Mode::Palette(palette) => palette.draw(canvas, self.font_scale, &app.commands),
            Mode::Tool(tool) => {
                tool::draw_frame(canvas, self.font_scale, tool.title())?;
                tool.draw(canvas, self.font_scale, app)
            }
        }
    }
}
