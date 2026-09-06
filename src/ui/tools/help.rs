//! Help page: key bindings and every registered command.

use crate::ui::key::Key;

use crate::ui::app::App;
use crate::ui::commands::NAME_COLUMNS;
use crate::ui::font;
use crate::ui::painter::{Color, Painter};
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

    fn handle_key(&mut self, key: Key, _app: &mut App) -> ToolEvent {
        match key {
            Key::Up => self.scroll = self.scroll.saturating_sub(1),
            Key::Down => self.scroll += 1,
            Key::PageUp => self.scroll = self.scroll.saturating_sub(10),
            Key::PageDown => self.scroll += 10,
            Key::Home => self.scroll = 0,
            Key::Char('q') | Key::Return => return ToolEvent::Close,
            _ => {}
        }
        ToolEvent::Continue
    }

    fn draw(&self, painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
        let lines = Help::lines(app);
        let (columns, rows) = tool::body_grid(painter, font_scale);
        // Never scroll past the point where the last line is at the top.
        let first = self.scroll.min(lines.len().saturating_sub(1));
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        for Line(text, colour) in lines.iter().skip(first).take(rows) {
            font::draw_text(
                painter,
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
