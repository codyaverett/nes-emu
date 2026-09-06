//! APU page: the five mixer channels and whether each is muted.
//!
//! Keys 1-5 toggle a channel, U unmutes all. The flags live in the
//! library's `Apu` (`set_channel_muted`), so the palette commands
//! `mute pulse1` .. `mute dmc` and `unmute all` change the same state
//! this page shows.

use crate::ui::key::Key;

use nes_emu::apu::{CHANNEL_COUNT, CHANNEL_NAMES};

use crate::ui::app::App;
use crate::ui::font;
use crate::ui::painter::Painter;
use crate::ui::tool::{self, Tool, ToolEvent};

pub struct ApuView;

/// Channel index for a digit key 1-5, if `key` is one.
fn channel_for_key(key: Key) -> Option<usize> {
    key.digit()
        .filter(|d| (1..=CHANNEL_COUNT as u8).contains(d))
        .map(|d| d as usize - 1)
}

impl Tool for ApuView {
    fn title(&self) -> &str {
        "APU"
    }

    fn handle_key(&mut self, key: Key, app: &mut App) -> ToolEvent {
        if let Some(ch) = channel_for_key(key) {
            app.toggle_channel_mute(ch);
        } else {
            match key {
                Key::Char('u') => app.unmute_all(),
                Key::Char('q') => return ToolEvent::Close,
                _ => {}
            }
        }
        ToolEvent::Continue
    }

    fn draw(&self, painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            tool::ACCENT,
            "Channel        State     Key",
        )?;
        y += step;
        for (ch, name) in CHANNEL_NAMES.iter().enumerate() {
            let muted = app.system.apu.channel_muted(ch);
            let (state, colour) = if muted {
                ("muted", tool::DIM_TEXT)
            } else {
                ("playing", tool::TEXT)
            };
            let line = format!("{name:<14} {state:<9} {}", ch + 1);
            font::draw_text(painter, x, y, font_scale, colour, &line)?;
            y += step;
        }
        y += step;
        let master = if app.is_muted() {
            "Master: muted (M)"
        } else {
            "Master: on"
        };
        font::draw_text(painter, x, y, font_scale, tool::TEXT, master)?;
        y += step;
        font::draw_text(
            painter,
            x,
            y,
            font_scale,
            tool::DIM_TEXT,
            "1-5 toggle a channel, U unmutes all",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_keys_map_to_channels() {
        assert_eq!(channel_for_key(Key::Char('1')), Some(0));
        assert_eq!(channel_for_key(Key::Char('5')), Some(4));
        assert_eq!(channel_for_key(Key::Char('6')), None);
        assert_eq!(channel_for_key(Key::Char('0')), None);
        assert_eq!(channel_for_key(Key::Char('u')), None);
    }
}
