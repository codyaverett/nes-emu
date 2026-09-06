//! Help page: key bindings and every registered command.

use sdl2::keyboard::Keycode;
use sdl2::pixels::Color;
use sdl2::render::WindowCanvas;

use crate::ui::app::App;
use crate::ui::commands::NAME_COLUMNS;
use crate::ui::font;
use crate::ui::tool::{self, Tool, ToolEvent};
use crate::ui::KEY_BINDINGS;

#[derive(Default)]
pub struct Help {
    /// First body line shown; Up/Down and PageUp/PageDown move it.
    scroll: usize,
}

/// One rendered line: text and colour.
struct Line(String, Color);

impl Help {
    fn lines(app: &App) -> Vec<Line> {
        let mut out = Vec::new();
        out.push(Line("Key bindings".into(), tool::ACCENT));
        for (key, what) in KEY_BINDINGS {
            out.push(Line(format!("{key:<13} {what}"), tool::TEXT));
        }
        out.push(Line(String::new(), tool::TEXT));
        // Recording state, live (issue #44): `rewind on` / `rewind off`.
        out.push(Line(app.rewind_status(), tool::DIM_TEXT));
        out.push(Line(String::new(), tool::TEXT));
        out.push(Line(
            "Commands (backquote, type to filter, Enter)".into(),
            tool::ACCENT,
        ));
        for c in &app.commands {
            out.push(Line(
                format!("{:<w$} {}", c.name, c.description, w = NAME_COLUMNS),
                tool::TEXT,
            ));
        }
        out.push(Line(String::new(), tool::TEXT));
        out.push(Line(
            "Up/Down scroll; Esc, Q or Return close".into(),
            tool::DIM_TEXT,
        ));
        out
    }
}

impl Tool for Help {
    fn title(&self) -> &str {
        "Help"
    }

    fn handle_key(&mut self, key: Keycode, _app: &mut App) -> ToolEvent {
        match key {
            Keycode::Up => self.scroll = self.scroll.saturating_sub(1),
            Keycode::Down => self.scroll += 1,
            Keycode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            Keycode::PageDown => self.scroll += 10,
            Keycode::Home => self.scroll = 0,
            Keycode::Q | Keycode::Return => return ToolEvent::Close,
            _ => {}
        }
        ToolEvent::Continue
    }

    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String> {
        let lines = Help::lines(app);
        let (columns, rows) = tool::body_grid(canvas, font_scale)?;
        // Never scroll past the point where the last line is at the top.
        let first = self.scroll.min(lines.len().saturating_sub(1));
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        for Line(text, colour) in lines.iter().skip(first).take(rows) {
            font::draw_text(
                canvas,
                x,
                y,
                font_scale,
                *colour,
                &tool::clip(text, columns),
            )?;
            y += step;
        }
        Ok(())
    }
}
