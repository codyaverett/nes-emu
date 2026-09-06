//! Memory page: a hex dump of CPU space read with `System::peek`.
//!
//! Sixteen bytes per row with an ASCII column, as many rows as fit. A
//! row is 72 characters, more than the 44 columns the window offers at
//! the UI font scale, so the dump is drawn at half the font scale (8 px
//! glyphs on the default window); the header keeps the normal size. The
//! first address lives in `App::memory_addr` so `mem ADDR` can set it and
//! it survives closing and reopening the page.

use std::cell::Cell;

use sdl2::keyboard::Keycode;
use sdl2::render::WindowCanvas;

use crate::ui::app::App;
use crate::ui::font;
use crate::ui::tool::{self, Tool, ToolEvent};

/// Bytes per row.
pub const ROW_BYTES: u16 = 16;

pub struct Memory {
    /// Rows drawn last frame, so paging keys know the page size before
    /// the next draw. Starts at a sensible guess.
    rows: Cell<u16>,
}

impl Default for Memory {
    fn default() -> Self {
        Memory {
            rows: Cell::new(32),
        }
    }
}

/// Largest row-aligned first address that still fills a page of `rows`.
fn last_page_start(rows: u16) -> u16 {
    0x10000u32
        .saturating_sub(rows as u32 * ROW_BYTES as u32)
        .min(0xFFF0) as u16
}

/// One dump row: address, sixteen hex bytes with a gap after eight, and
/// the printable ASCII with `.` for everything else.
pub fn format_row(addr: u16, bytes: &[u8]) -> String {
    let mut line = format!("{addr:04X}  ");
    for (i, b) in bytes.iter().enumerate() {
        if i == 8 {
            line.push(' ');
        }
        line.push_str(&format!("{b:02X} "));
    }
    line.push(' ');
    for &b in bytes {
        line.push(if (0x20..0x7F).contains(&b) {
            b as char
        } else {
            '.'
        });
    }
    line
}

impl Tool for Memory {
    fn title(&self) -> &str {
        "Memory"
    }

    fn handle_key(&mut self, key: Keycode, app: &mut App) -> ToolEvent {
        let rows = self.rows.get().max(1);
        let page = rows.saturating_mul(ROW_BYTES);
        let limit = last_page_start(rows);
        let addr = app.memory_addr;
        app.memory_addr = match key {
            Keycode::Up => addr.saturating_sub(ROW_BYTES),
            Keycode::Down => addr.saturating_add(ROW_BYTES).min(limit),
            Keycode::PageUp => addr.saturating_sub(page),
            Keycode::PageDown => addr.saturating_add(page).min(limit),
            Keycode::Home => 0,
            Keycode::End => limit,
            Keycode::Q => return ToolEvent::Close,
            _ => addr,
        } & 0xFFF0;
        ToolEvent::Continue
    }

    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String> {
        let (_, h) = canvas.output_size()?;
        let dense = (font_scale / 2).max(1);
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(dense);

        // Rows that fit under the header line.
        let body_start = y + tool::line_step(font_scale);
        let avail = h as i32 - body_start - tool::padding(font_scale);
        let rows = ((avail / step).max(1) as u16).min(0x1000);
        self.rows.set(rows);

        let first = app.memory_addr.min(last_page_start(rows)) & 0xFFF0;
        let last = (first as u32 + rows as u32 * ROW_BYTES as u32 - 1).min(0xFFFF) as u16;
        let header = format!("{first:04X}-{last:04X}  Up/Down PgUp/PgDn Home/End");
        font::draw_text(canvas, x, y, font_scale, tool::ACCENT, &header)?;
        y = body_start;

        let mut buf = [0u8; ROW_BYTES as usize];
        for row in 0..rows {
            let addr = first as u32 + row as u32 * ROW_BYTES as u32;
            if addr > 0xFFFF {
                break;
            }
            for (i, b) in buf.iter_mut().enumerate() {
                *b = app.system.peek(addr as u16 + i as u16);
            }
            // $2000-$5FFF are registers `peek` refuses to touch; dim them.
            let colour = if (0x2000..0x6000).contains(&addr) {
                tool::DIM_TEXT
            } else {
                tool::TEXT
            };
            let line = format_row(addr as u16, &buf);
            font::draw_text(canvas, x, y, dense, colour, &line)?;
            y += step;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_seventy_two_columns() {
        let bytes: Vec<u8> = (0x41..0x51).collect();
        let line = format_row(0x0200, &bytes);
        assert_eq!(line.len(), 72);
        assert!(line.starts_with("0200  41 42 43 44 45 46 47 48  49 4A"));
        assert!(line.ends_with("ABCDEFGHIJKLMNOP"));
        let line = format_row(0xFFF0, &[0u8; 16]);
        assert!(line.ends_with("................"));
    }

    #[test]
    fn last_page_never_runs_past_the_top_of_memory() {
        assert_eq!(last_page_start(1), 0xFFF0);
        assert_eq!(last_page_start(60), (0x10000u32 - 60 * 16) as u16);
        assert_eq!(last_page_start(0x1000), 0);
        assert_eq!(last_page_start(0xFFFF), 0);
    }
}
