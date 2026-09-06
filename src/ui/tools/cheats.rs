//! Cheats page: list, toggle, add, delete and describe cheats.
//!
//! The list shows one row per cheat: its 1-based number (the `N` that
//! `cheat toggle N` takes), an enabled marker, the code and the
//! description. Up/Down move the cursor, Space toggles, D deletes, A adds
//! (an inline text entry asks for the code, then the description), E
//! edits the description of the selected cheat. Every change goes through
//! the `App` cheat methods, which rewrite the `.cht` file. A rejected code
//! is shown in red below the list and the page stays open.
//!
//! Text entry reads characters from `Key::Char` like the palette does, so
//! Shift is ignored (codes are case-insensitive) and `:` / `?` are typed
//! as `;` / `/`; `App::add_cheat` maps them back.

use crate::ui::key::Key;

use crate::ui::app::App;
use crate::ui::font;
use crate::ui::painter::{Color, Painter};
use crate::ui::tool::{self, Tool, ToolEvent, ACCENT, DIM_TEXT, TEXT};

const ERROR: Color = Color::rgb(255, 96, 96);
const SELECTED: Color = Color::rgba(60, 90, 170, 255);
const ENABLED: Color = Color::rgb(120, 230, 120);

/// Widest code the list column reserves; longer codes push the
/// description right.
const CODE_COLUMNS: usize = 11;

/// What the keyboard is currently doing.
enum Mode {
    /// Keys move the cursor and act on the selected cheat.
    List,
    /// Typing a new code.
    EnterCode { buffer: String },
    /// Typing the description of a code that already parsed.
    EnterDescription { code: String, buffer: String },
    /// Replacing the description of an existing cheat.
    EditDescription { index: usize, buffer: String },
}

pub struct Cheats {
    /// Selected row. The list scrolls so this row is always visible.
    cursor: usize,
    mode: Mode,
}

impl Default for Cheats {
    fn default() -> Self {
        Cheats {
            cursor: 0,
            mode: Mode::List,
        }
    }
}

impl Cheats {
    fn clamp_cursor(&mut self, app: &App) {
        let len = app.system.cheats().len();
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }

    fn handle_list_key(&mut self, key: Key, app: &mut App) -> ToolEvent {
        let len = app.system.cheats().len();
        match key {
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down => {
                if self.cursor + 1 < len {
                    self.cursor += 1;
                }
            }
            Key::PageUp => self.cursor = self.cursor.saturating_sub(10),
            Key::PageDown => self.cursor = (self.cursor + 10).min(len.saturating_sub(1)),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = len.saturating_sub(1),
            Key::Char(' ') | Key::Return => {
                if len > 0 {
                    app.toggle_cheat(self.cursor);
                }
            }
            Key::Char('d') | Key::Delete | Key::Backspace => {
                if len > 0 {
                    app.remove_cheat(self.cursor);
                    self.clamp_cursor(app);
                }
            }
            Key::Char('a') | Key::Insert => {
                self.mode = Mode::EnterCode {
                    buffer: String::new(),
                };
            }
            Key::Char('e') => {
                if let Some(cheat) = app.system.cheats().get(self.cursor) {
                    self.mode = Mode::EditDescription {
                        index: self.cursor,
                        buffer: cheat.description.clone(),
                    };
                }
            }
            Key::Char('q') => return ToolEvent::Close,
            _ => {}
        }
        ToolEvent::Continue
    }

    /// Shared editing of the entry buffer; returns true when Enter was
    /// pressed and the caller should act on the text.
    fn edit_buffer(buffer: &mut String, key: Key) -> bool {
        match key {
            Key::Return => return true,
            Key::Backspace => {
                buffer.pop();
            }
            _ => {
                if let Some(ch) = printable(key) {
                    buffer.push(ch);
                }
            }
        }
        false
    }

    fn handle_entry_key(&mut self, key: Key, app: &mut App) {
        if key == Key::Escape {
            // Cancelling also drops the error the entry produced.
            app.cheat_error = None;
            self.mode = Mode::List;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::List);
        self.mode = match mode {
            Mode::List => Mode::List,
            Mode::EnterCode { mut buffer } => {
                if !Self::edit_buffer(&mut buffer, key) {
                    Mode::EnterCode { buffer }
                } else if buffer.trim().is_empty() {
                    app.cheat_error = Some("Type a code first".into());
                    Mode::EnterCode { buffer }
                } else {
                    // Parse now so a bad code is reported before the
                    // description is typed; the App parses again on add.
                    let code = buffer.replace(';', ":").replace('/', "?");
                    match nes_emu::cheat::Cheat::parse(&code) {
                        Ok(_) => {
                            app.cheat_error = None;
                            Mode::EnterDescription {
                                code,
                                buffer: String::new(),
                            }
                        }
                        Err(e) => {
                            log::warn!("Cheat {:?} rejected: {}", code, e);
                            app.cheat_error = Some(format!("{}: {}", code.trim(), e));
                            Mode::EnterCode { buffer }
                        }
                    }
                }
            }
            Mode::EnterDescription { code, mut buffer } => {
                if !Self::edit_buffer(&mut buffer, key) {
                    Mode::EnterDescription { code, buffer }
                } else {
                    match app.add_cheat(&code, &buffer) {
                        Ok(index) => {
                            self.cursor = index;
                            Mode::List
                        }
                        Err(_) => Mode::EnterCode { buffer: code },
                    }
                }
            }
            Mode::EditDescription { index, mut buffer } => {
                if !Self::edit_buffer(&mut buffer, key) {
                    Mode::EditDescription { index, buffer }
                } else {
                    app.set_cheat_description(index, &buffer);
                    Mode::List
                }
            }
        };
    }

    /// The entry prompt and its text, if an entry is open.
    fn entry_line(&self) -> Option<(&'static str, &str)> {
        match &self.mode {
            Mode::List => None,
            Mode::EnterCode { buffer } => Some(("Code: ", buffer)),
            Mode::EnterDescription { buffer, .. } => Some(("Description: ", buffer)),
            Mode::EditDescription { buffer, .. } => Some(("Edit description: ", buffer)),
        }
    }
}

impl Tool for Cheats {
    fn title(&self) -> &str {
        "Cheats"
    }

    fn captures_escape(&self) -> bool {
        !matches!(self.mode, Mode::List)
    }

    fn handle_key(&mut self, key: Key, app: &mut App) -> ToolEvent {
        self.clamp_cursor(app);
        match self.mode {
            Mode::List => self.handle_list_key(key, app),
            _ => {
                self.handle_entry_key(key, app);
                ToolEvent::Continue
            }
        }
    }

    fn tick(&mut self, app: &mut App) {
        // The palette can change the set while the page is closed, and
        // `cheat clear` while it is open, so keep the cursor in range.
        self.clamp_cursor(app);
    }

    fn draw(&self, painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
        let (columns, rows) = tool::body_grid(painter, font_scale);
        let (window_w, _) = painter.size();
        let pad = tool::padding(font_scale);
        let x = pad;
        let step = tool::line_step(font_scale);
        let mut y = tool::body_top(font_scale);
        let cheats = app.system.cheats();

        // Header, then the list, then up to three footer lines: the entry
        // prompt, the error, and the key hint.
        let footer_rows = 3;
        let list_rows = rows.saturating_sub(1 + footer_rows).max(1);

        let enabled = cheats.iter().filter(|c| c.enabled).count();
        let header = format!(
            "{} cheat(s), {} enabled   file: {}",
            cheats.len(),
            enabled,
            app.cheat_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            ACCENT,
            &tool::clip(&header, columns),
        )?;
        y += step;

        if cheats.is_empty() {
            font::draw_text(
                painter,
                x,
                y,
                font_scale,
                DIM_TEXT,
                &tool::clip("No cheats. Press A to add one.", columns),
            )?;
            y += step * list_rows as i32;
        } else {
            // Scroll just enough to keep the cursor on the last visible row.
            let first = (self.cursor + 1).saturating_sub(list_rows);
            for (index, cheat) in cheats.iter().enumerate().skip(first).take(list_rows) {
                let selected = index == self.cursor;
                if selected {
                    painter.fill_rect(
                        pad / 2,
                        y - font_scale as i32,
                        window_w.saturating_sub(pad as u32),
                        step as u32,
                        SELECTED,
                    )?;
                }
                let marker = if cheat.enabled { "[x]" } else { "[ ]" };
                let number = format!("{:>2} ", index + 1);
                let rest = format!(
                    " {:<w$} {}",
                    cheat.code,
                    cheat.description,
                    w = CODE_COLUMNS
                );
                let text_colour = if selected { TEXT } else { DIM_TEXT };
                let marker_colour = if cheat.enabled { ENABLED } else { DIM_TEXT };
                let mut cx = x;
                font::draw_text(painter, cx, y, font_scale, text_colour, &number)?;
                cx += font::text_width(&number, font_scale) as i32;
                font::draw_text(painter, cx, y, font_scale, marker_colour, marker)?;
                cx += font::text_width(marker, font_scale) as i32;
                let used = number.len() + marker.len();
                font::draw_text(
                    painter,
                    cx,
                    y,
                    font_scale,
                    text_colour,
                    &tool::clip(&rest, columns.saturating_sub(used)),
                )?;
                y += step;
            }
            // Pad to a fixed height so the footer does not jump.
            let shown = cheats.len().saturating_sub(first).min(list_rows);
            y += step * (list_rows - shown) as i32;
        }

        if let Some((prompt, text)) = self.entry_line() {
            let line = format!("{}{}_", prompt, text);
            font::draw_text(
                painter,
                x,
                y,
                font_scale,
                ACCENT,
                &tool::clip(&line, columns),
            )?;
        }
        y += step;

        if let Some(error) = &app.cheat_error {
            font::draw_text(
                painter,
                x,
                y,
                font_scale,
                ERROR,
                &tool::clip(error, columns),
            )?;
        }
        y += step;

        let hint = match self.mode {
            Mode::List => "Space toggle A add D del E edit Esc close",
            _ => "Enter confirms, Esc cancels",
        };
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            DIM_TEXT,
            &tool::clip(hint, columns),
        )
    }
}

/// The character a key press types, as in the palette: printable ASCII
/// `Key::printable`, Shift is ignored.
fn printable(key: Key) -> Option<char> {
    key.printable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_buffer_types_and_deletes() {
        let mut buffer = String::new();
        assert!(!Cheats::edit_buffer(&mut buffer, Key::Char('s')));
        assert!(!Cheats::edit_buffer(&mut buffer, Key::Char('x')));
        assert!(!Cheats::edit_buffer(&mut buffer, Key::Char(' ')));
        assert!(!Cheats::edit_buffer(&mut buffer, Key::Char(';')));
        assert!(!Cheats::edit_buffer(&mut buffer, Key::F1));
        assert_eq!(buffer, "sx ;");
        assert!(!Cheats::edit_buffer(&mut buffer, Key::Backspace));
        assert_eq!(buffer, "sx ");
        assert!(Cheats::edit_buffer(&mut buffer, Key::Return));
        assert_eq!(buffer, "sx ");
    }

    #[test]
    fn escape_is_captured_only_while_entering_text() {
        let mut page = Cheats::default();
        assert!(!page.captures_escape());
        page.mode = Mode::EnterCode {
            buffer: String::new(),
        };
        assert!(page.captures_escape());
        page.mode = Mode::EditDescription {
            index: 0,
            buffer: String::new(),
        };
        assert!(page.captures_escape());
    }
}
