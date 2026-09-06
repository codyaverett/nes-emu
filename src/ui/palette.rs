//! Command palette overlay.
//!
//! Typing filters the registry with a case-insensitive subsequence match,
//! Up/Down move the selection, Enter runs it, Escape (or the backquote
//! that opened it) closes. Characters come from `Key::Char` rather than SDL
//! text-input events so the scripted `--ui-script` path behaves exactly
//! like a real keyboard.

use super::key::Key;

use super::commands::{self, Command};
use super::font;
use super::painter::{Color, Painter};
use super::tool::{self, ACCENT, DIM_TEXT, TEXT};

/// Matches shown at once; the list scrolls to keep the selection visible.
pub const MAX_ROWS: usize = 8;

const PANEL: Color = Color::rgba(10, 12, 28, 220);
const SELECTED: Color = Color::rgba(60, 90, 170, 255);

pub enum PaletteEvent {
    Continue,
    Close,
    /// Run the command at this registry index with the text typed after
    /// its name (trimmed, possibly empty) and close.
    Run(usize, String),
}

#[derive(Default)]
pub struct Palette {
    input: String,
    /// Index into the current match list.
    selected: usize,
}

impl Palette {
    pub fn new() -> Self {
        Palette::default()
    }

    /// Registry indices matching the current input.
    pub fn matches(&self, registry: &[Command]) -> Vec<usize> {
        commands::filter(registry, &self.input)
    }

    pub fn handle_key(&mut self, key: Key, registry: &[Command]) -> PaletteEvent {
        let matches = self.matches(registry);
        match key {
            Key::Escape | Key::Backquote => return PaletteEvent::Close,
            Key::Return => {
                return match matches.get(self.selected) {
                    Some(&index) => PaletteEvent::Run(
                        index,
                        commands::argument(&self.input, registry[index].name),
                    ),
                    None => PaletteEvent::Continue,
                };
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => {
                if self.selected + 1 < matches.len() {
                    self.selected += 1;
                }
            }
            Key::Backspace => {
                self.input.pop();
                self.selected = 0;
            }
            _ => {
                if let Some(ch) = printable(key) {
                    self.input.push(ch);
                    self.selected = 0;
                }
            }
        }
        PaletteEvent::Continue
    }

    pub fn draw(
        &self,
        painter: &mut dyn Painter,
        font_scale: u32,
        registry: &[Command],
    ) -> Result<(), String> {
        let (window_w, _) = painter.size();
        let matches = self.matches(registry);
        let pad = tool::padding(font_scale);
        let line = font::line_height(font_scale) as i32;
        let row_h = line + pad;
        let visible = matches.len().clamp(1, MAX_ROWS);
        let panel_x = pad;
        let panel_y = pad;
        let panel_w = (window_w as i32 - 2 * pad).max(0) as u32;
        let panel_h = (pad + row_h + pad + visible as i32 * row_h + pad) as u32;
        let columns = ((panel_w as i32 - 2 * pad) / line).max(1) as usize;

        painter.fill_rect(panel_x, panel_y, panel_w, panel_h, PANEL)?;

        // Input line with a block cursor.
        let text_x = panel_x + pad;
        let mut y = panel_y + pad;
        let prompt = format!("> {}_", self.input);
        font::draw_text(
            painter,
            text_x,
            y,
            font_scale,
            ACCENT,
            &tool::clip(&prompt, columns),
        )?;
        y += row_h;
        painter.fill_rect(text_x, y, panel_w - 2 * pad as u32, 1, DIM_TEXT)?;
        y += pad;

        if matches.is_empty() {
            return font::draw_text(
                painter,
                text_x,
                y,
                font_scale,
                DIM_TEXT,
                "no matching commands",
            );
        }

        // Keep the selection on screen.
        let first = self.selected.saturating_sub(MAX_ROWS - 1);
        let name_cols = commands::NAME_COLUMNS;
        for (row, &index) in matches.iter().enumerate().skip(first).take(MAX_ROWS) {
            let command = &registry[index];
            let is_selected = row == self.selected;
            if is_selected {
                painter.fill_rect(
                    panel_x + pad / 2,
                    y - pad / 2,
                    panel_w - pad as u32,
                    row_h as u32,
                    SELECTED,
                )?;
            }
            let text = format!("{:<name_cols$} {}", command.name, command.description);
            let colour = if is_selected { TEXT } else { DIM_TEXT };
            font::draw_text(
                painter,
                text_x,
                y,
                font_scale,
                colour,
                &tool::clip(&text, columns),
            )?;
            y += row_h;
        }
        Ok(())
    }
}

/// The character a key press types into the input, if any (see
/// `Key::printable`; Shift is ignored since matching is case-insensitive).
fn printable(key: Key) -> Option<char> {
    key.printable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::commands::builtin_commands;

    fn names(palette: &Palette, registry: &[Command]) -> Vec<&'static str> {
        palette
            .matches(registry)
            .into_iter()
            .map(|i| registry[i].name)
            .collect()
    }

    #[test]
    fn typing_filters_and_resets_selection() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        assert_eq!(names(&p, &registry).len(), registry.len());
        p.handle_key(Key::Down, &registry);
        assert_eq!(p.selected, 1);
        for k in [Key::Char('v'), Key::Char('o'), Key::Char('l')] {
            p.handle_key(k, &registry);
        }
        assert_eq!(p.input, "vol");
        assert_eq!(p.selected, 0);
        assert_eq!(names(&p, &registry), ["volume up", "volume down"]);
        p.handle_key(Key::Backspace, &registry);
        assert_eq!(p.input, "vo");
    }

    #[test]
    fn enter_runs_selected_match() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for k in [Key::Char('v'), Key::Char('o'), Key::Char('l'), Key::Down] {
            p.handle_key(k, &registry);
        }
        match p.handle_key(Key::Return, &registry) {
            PaletteEvent::Run(i, arg) => {
                assert_eq!(registry[i].name, "volume down");
                assert_eq!(arg, "");
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn enter_passes_the_argument_after_the_name() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for ch in "cheat add sxiopo".chars() {
            p.handle_key(Key::Char(ch), &registry);
        }
        assert_eq!(names(&p, &registry), ["cheat add"]);
        match p.handle_key(Key::Return, &registry) {
            PaletteEvent::Run(i, arg) => {
                assert_eq!(registry[i].name, "cheat add");
                assert_eq!(arg, "sxiopo");
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn enter_passes_the_argument_text() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for k in [
            Key::Char('m'),
            Key::Char('e'),
            Key::Char('m'),
            Key::Char(' '),
            Key::Char('c'),
            Key::Char('0'),
            Key::Char('0'),
            Key::Char('0'),
        ] {
            p.handle_key(k, &registry);
        }
        assert_eq!(names(&p, &registry), ["mem"]);
        match p.handle_key(Key::Return, &registry) {
            PaletteEvent::Run(i, arg) => {
                assert_eq!(registry[i].name, "mem");
                assert_eq!(arg, "c000");
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn selection_clamps_and_escape_closes() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for _ in 0..50 {
            p.handle_key(Key::Down, &registry);
        }
        assert_eq!(p.selected, registry.len() - 1);
        p.handle_key(Key::Up, &registry);
        assert_eq!(p.selected, registry.len() - 2);
        assert!(matches!(
            p.handle_key(Key::Escape, &registry),
            PaletteEvent::Close
        ));
        assert!(matches!(
            p.handle_key(Key::Backquote, &registry),
            PaletteEvent::Close
        ));
    }

    #[test]
    fn enter_with_no_match_does_nothing() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for k in [Key::Char('q'), Key::Char('q'), Key::Char('q')] {
            p.handle_key(k, &registry);
        }
        assert!(names(&p, &registry).is_empty());
        assert!(matches!(
            p.handle_key(Key::Return, &registry),
            PaletteEvent::Continue
        ));
    }

    #[test]
    fn printable_maps_ascii_keys() {
        assert_eq!(printable(Key::Char('a')), Some('a'));
        assert_eq!(printable(Key::Char(' ')), Some(' '));
        assert_eq!(printable(Key::Char('3')), Some('3'));
        assert_eq!(printable(Key::Backquote), None);
        assert_eq!(printable(Key::F1), None);
        assert_eq!(printable(Key::Return), None);
    }

    #[test]
    fn browser_codes_type_into_the_palette() {
        let registry = builtin_commands();
        let mut p = Palette::new();
        for code in ["KeyV", "KeyO", "KeyL", "ShiftLeft", "Space", "Digit1"] {
            p.handle_key(Key::from_browser_code(code), &registry);
        }
        assert_eq!(p.input, "vol 1");
        assert!(matches!(
            p.handle_key(Key::from_browser_code("Backquote"), &registry),
            PaletteEvent::Close
        ));
    }
}
