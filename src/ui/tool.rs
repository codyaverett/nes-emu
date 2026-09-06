//! Tool pages: full-window overlays with their own key handling.
//!
//! A tool implements [`Tool`]; the `Ui` owns the open one, draws the
//! backdrop and title bar with [`draw_frame`], then calls `draw` for the
//! body, which starts at [`body_top`]. Escape closes the open tool (the
//! `Ui` handles it before the tool sees the key) unless the tool reports
//! [`Tool::captures_escape`], which a page does while an inline text entry
//! is open so Escape cancels the entry instead; a tool can also close
//! itself by returning [`ToolEvent::Close`].

use sdl2::keyboard::Keycode;
use sdl2::pixels::Color;
use sdl2::rect::Rect;
use sdl2::render::WindowCanvas;

use super::app::App;
use super::font;

pub enum ToolEvent {
    Continue,
    Close,
}

pub trait Tool {
    fn title(&self) -> &str;

    /// Handle one key press. Escape only arrives while
    /// [`Tool::captures_escape`] returns true.
    fn handle_key(&mut self, key: Keycode, app: &mut App) -> ToolEvent;

    /// True while the tool wants Escape delivered to `handle_key` instead
    /// of closing the page (an inline text entry is open).
    fn captures_escape(&self) -> bool {
        false
    }

    /// Draw the body below the title bar; see [`body_top`] and [`padding`].
    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String>;

    /// Called once per presented frame while the tool is open.
    fn tick(&mut self, _app: &mut App) {}
}

/// Backdrop colour for tool pages: dark enough for the text to read, thin
/// enough that the game frame stays recognisable behind it.
pub const BACKDROP: Color = Color::RGBA(8, 10, 24, 236);
pub const TITLE_BAR: Color = Color::RGBA(40, 56, 110, 255);
pub const TEXT: Color = Color::RGB(230, 230, 230);
pub const DIM_TEXT: Color = Color::RGB(150, 155, 170);
pub const ACCENT: Color = Color::RGB(255, 220, 120);

/// Left and top margin of text inside a page, in window pixels.
pub fn padding(font_scale: u32) -> i32 {
    (4 * font_scale) as i32
}

/// Height of the title bar in window pixels.
pub fn title_height(font_scale: u32) -> u32 {
    font::line_height(font_scale) + 2 * padding(font_scale) as u32
}

/// Vertical distance between text rows in a page body: one glyph plus
/// two font pixels of leading so descenders never touch the next row.
pub fn line_step(font_scale: u32) -> i32 {
    (font::line_height(font_scale) + 2 * font_scale) as i32
}

/// Y coordinate where a tool's body starts.
pub fn body_top(font_scale: u32) -> i32 {
    title_height(font_scale) as i32 + padding(font_scale)
}

/// Fill the window with the backdrop and draw the title bar.
pub fn draw_frame(canvas: &mut WindowCanvas, font_scale: u32, title: &str) -> Result<(), String> {
    let (w, h) = canvas.output_size()?;
    canvas.set_draw_color(BACKDROP);
    canvas.fill_rect(Rect::new(0, 0, w, h))?;
    canvas.set_draw_color(TITLE_BAR);
    canvas.fill_rect(Rect::new(0, 0, w, title_height(font_scale)))?;
    let pad = padding(font_scale);
    font::draw_text(canvas, pad, pad, font_scale, TEXT, title)?;
    let hint = "Esc closes";
    let hint_x = w as i32 - pad - font::text_width(hint, font_scale) as i32;
    font::draw_text(canvas, hint_x, pad, font_scale, DIM_TEXT, hint)
}

/// Number of text columns and body rows that fit in the window.
pub fn body_grid(canvas: &WindowCanvas, font_scale: u32) -> Result<(usize, usize), String> {
    let (w, h) = canvas.output_size()?;
    let pad = padding(font_scale) as u32;
    let cell = font::line_height(font_scale);
    let columns = w.saturating_sub(2 * pad) / cell;
    let rows = h.saturating_sub(body_top(font_scale) as u32 + pad) / line_step(font_scale) as u32;
    Ok((columns as usize, rows as usize))
}

/// Shorten `text` to at most `columns` characters.
pub fn clip(text: &str, columns: usize) -> String {
    text.chars().take(columns).collect()
}
