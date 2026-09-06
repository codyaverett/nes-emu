//! States page: the nine save-state slots with their file times.
//!
//! One row per slot showing whether the slot is used, when it was last
//! written (UTC) and its size, as reported by the `Host` (`<rom>.sN`
//! files in the SDL binary, the browser store on the web). Up/Down move
//! the highlight, Return loads the highlighted slot, S saves to it; both
//! make it the current slot (the one F5/F8 use). The list is re-read on
//! every draw, so a save made from the page shows up at once. Escape or
//! Q close.

use crate::ui::app::{App, STATE_SLOTS};
use crate::ui::font;
use crate::ui::host::SlotInfo;
use crate::ui::key::Key;
use crate::ui::painter::Painter;
use crate::ui::tool::{self, Tool, ToolEvent};

#[derive(Default)]
pub struct States {
    /// Highlighted slot, 1-based; `None` means the current slot.
    cursor: Option<u8>,
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
                .modified_unix_secs
                .map(format_utc)
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

    fn handle_key(&mut self, key: Key, app: &mut App) -> ToolEvent {
        let cursor = self.cursor(app);
        match key {
            Key::Up => self.cursor = Some(if cursor <= 1 { STATE_SLOTS } else { cursor - 1 }),
            Key::Down => self.cursor = Some(if cursor >= STATE_SLOTS { 1 } else { cursor + 1 }),
            Key::Home => self.cursor = Some(1),
            Key::End => self.cursor = Some(STATE_SLOTS),
            Key::Return => app.load_state_from(cursor),
            Key::Char('s') => app.save_state_to(cursor),
            Key::Char('q') => return ToolEvent::Close,
            _ => {
                // Digits jump straight to a slot.
                if let Some(d) = key.digit().filter(|d| (1..=STATE_SLOTS).contains(d)) {
                    self.cursor = Some(d);
                }
            }
        }
        ToolEvent::Continue
    }

    fn draw(&self, painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        let cursor = self.cursor(app);
        let file = app.host.slot_label(app.slot);
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            tool::ACCENT,
            &format!("Current slot {} ({})", app.slot, file),
        )?;
        y += step;
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "  #  Written (UTC)             Size",
        )?;
        y += step;
        for slot in 1..=STATE_SLOTS {
            let info = app.host.slot_info(slot);
            let row = format_row(slot, info.as_ref(), slot == app.slot);
            let colour = if slot == cursor {
                tool::ACCENT
            } else if info.is_some() {
                tool::TEXT
            } else {
                tool::DIM_TEXT
            };
            font::draw_text(painter, x, y, font_scale, colour, &row)?;
            y += step;
        }
        y += step;
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "Up/Down or 1-9 pick, Enter loads, S saves",
        )?;
        y += step;
        font::draw_text(
            painter,
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
            modified_unix_secs: Some(1_700_000_000),
            size: 15098,
        };
        let row = format_row(3, Some(&info), true);
        assert_eq!(row, "> 3  2023-11-14 22:13:20 UTC   15098 B");
        assert!(row.len() <= 44);
        assert_eq!(format_row(9, None, false), "  9  empty");
    }
}
