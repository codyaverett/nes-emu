//! States page: the nine save-state slots with their file times.
//!
//! One row per slot showing whether `<rom>.sN` exists, when it was last
//! written (UTC) and its size. Up/Down move the highlight, Return loads
//! the highlighted slot, S saves to it; both make it the current slot
//! (the one F5/F8 use). The list is re-read from disk on every draw, so a
//! save made from the page shows up at once. Escape or Q close.

use std::time::{SystemTime, UNIX_EPOCH};

use sdl2::keyboard::Keycode;
use sdl2::render::WindowCanvas;

use crate::ui::app::{App, STATE_SLOTS};
use crate::ui::font;
use crate::ui::tool::{self, Tool, ToolEvent};

#[derive(Default)]
pub struct States {
    /// Highlighted slot, 1-based; `None` means the current slot.
    cursor: Option<u8>,
}

/// What the page knows about one slot file.
pub struct SlotInfo {
    modified: Option<SystemTime>,
    size: u64,
}

fn slot_info(app: &App, slot: u8) -> Option<SlotInfo> {
    let meta = std::fs::metadata(app.state_path(slot)).ok()?;
    Some(SlotInfo {
        modified: meta.modified().ok(),
        size: meta.len(),
    })
}

/// `YYYY-MM-DD HH:MM:SS` in UTC from seconds since the Unix epoch
/// (Howard Hinnant's days-to-civil algorithm; no time-zone crate).
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// One list row: `N  2026-09-05 12:34:56 UTC  15098 B` or `N  empty`.
pub fn format_row(slot: u8, info: Option<&SlotInfo>, current: bool) -> String {
    let mark = if current { '>' } else { ' ' };
    match info {
        Some(info) => {
            let when = info
                .modified
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| format_utc(d.as_secs()))
                .unwrap_or_else(|| "unknown time".to_string());
            format!("{mark} {slot}  {when} UTC  {:>6} B", info.size)
        }
        None => format!("{mark} {slot}  empty"),
    }
}

impl States {
    fn cursor(&self, app: &App) -> u8 {
        self.cursor.unwrap_or(app.slot)
    }
}

impl Tool for States {
    fn title(&self) -> &str {
        "Save states"
    }

    fn handle_key(&mut self, key: Keycode, app: &mut App) -> ToolEvent {
        let cursor = self.cursor(app);
        match key {
            Keycode::Up => self.cursor = Some(if cursor <= 1 { STATE_SLOTS } else { cursor - 1 }),
            Keycode::Down => self.cursor = Some(if cursor >= STATE_SLOTS { 1 } else { cursor + 1 }),
            Keycode::Home => self.cursor = Some(1),
            Keycode::End => self.cursor = Some(STATE_SLOTS),
            Keycode::Return => app.load_state_from(cursor),
            Keycode::S => app.save_state_to(cursor),
            Keycode::Q => return ToolEvent::Close,
            _ => {
                // Digits jump straight to a slot.
                let code = key as i32;
                if (b'1' as i32..=b'9' as i32).contains(&code) {
                    self.cursor = Some((code - b'0' as i32) as u8);
                }
            }
        }
        ToolEvent::Continue
    }

    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String> {
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        let cursor = self.cursor(app);
        let file = app
            .state_path(app.slot)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        font::draw_text(
            canvas,
            x,
            y,
            font_scale,
            tool::ACCENT,
            &format!("Current slot {} ({})", app.slot, file),
        )?;
        y += step;
        font::draw_text(
            canvas,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "  #  Written (UTC)             Size",
        )?;
        y += step;
        for slot in 1..=STATE_SLOTS {
            let info = slot_info(app, slot);
            let row = format_row(slot, info.as_ref(), slot == app.slot);
            let colour = if slot == cursor {
                tool::ACCENT
            } else if info.is_some() {
                tool::TEXT
            } else {
                tool::DIM_TEXT
            };
            font::draw_text(canvas, x, y, font_scale, colour, &row)?;
            y += step;
        }
        y += step;
        font::draw_text(
            canvas,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "Up/Down or 1-9 pick, Enter loads, S saves",
        )?;
        y += step;
        font::draw_text(
            canvas,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "In game: F5 save, F8 load, F6/F7 slot",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(format_utc(1_772_323_199), "2026-02-28 23:59:59");
        assert_eq!(format_utc(1_772_323_200), "2026-03-01 00:00:00");
    }

    #[test]
    fn rows_fit_the_window() {
        let info = SlotInfo {
            modified: Some(UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            size: 15098,
        };
        let row = format_row(3, Some(&info), true);
        assert_eq!(row, "> 3  2023-11-14 22:13:20 UTC   15098 B");
        assert!(row.len() <= 44);
        assert_eq!(format_row(9, None, false), "  9  empty");
    }
}
