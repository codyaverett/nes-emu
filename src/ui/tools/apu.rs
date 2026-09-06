//! APU page: the five mixer channels and whether each is muted.
//!
//! Keys 1-5 toggle a channel, U unmutes all. The flags live in the
//! library's `Apu` (`set_channel_muted`), so the palette commands
//! `mute pulse1` .. `mute dmc` and `unmute all` change the same state
//! this page shows.

use sdl2::keyboard::Keycode;
use sdl2::render::WindowCanvas;

use nes_emu::apu::{CHANNEL_COUNT, CHANNEL_NAMES};

use crate::ui::app::App;
use crate::ui::font;
use crate::ui::tool::{self, Tool, ToolEvent};

pub struct ApuView;

/// Channel index for a digit key 1-5, if `key` is one.
fn channel_for_key(key: Keycode) -> Option<usize> {
    let code = key as i32;
    let ch = (code - b'1' as i32) as usize;
    ((b'1' as i32..b'1' as i32 + CHANNEL_COUNT as i32).contains(&code)).then_some(ch)
}

impl Tool for ApuView {
    fn title(&self) -> &str {
        "APU"
    }

    fn handle_key(&mut self, key: Keycode, app: &mut App) -> ToolEvent {
        if let Some(ch) = channel_for_key(key) {
            app.toggle_channel_mute(ch);
        } else {
            match key {
                Keycode::U => app.unmute_all(),
                Keycode::Q => return ToolEvent::Close,
                _ => {}
            }
        }
        ToolEvent::Continue
    }

    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String> {
        let x = tool::padding(font_scale);
        let mut y = tool::body_top(font_scale);
        let step = tool::line_step(font_scale);
        font::draw_text(
            canvas,
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
            font::draw_text(canvas, x, y, font_scale, colour, &line)?;
            y += step;
        }
        y += step;
        let master = if app.is_muted() {
            "Master: muted (M)"
        } else {
            "Master: on"
        };
        font::draw_text(canvas, x, y, font_scale, tool::TEXT, master)?;
        y += step;
        font::draw_text(
            canvas,
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
        assert_eq!(channel_for_key(Keycode::Num1), Some(0));
        assert_eq!(channel_for_key(Keycode::Num5), Some(4));
        assert_eq!(channel_for_key(Keycode::Num6), None);
        assert_eq!(channel_for_key(Keycode::Num0), None);
        assert_eq!(channel_for_key(Keycode::U), None);
    }
}
