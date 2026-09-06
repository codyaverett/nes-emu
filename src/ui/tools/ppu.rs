//! PPU page: pattern tables, nametables and palettes, read live from
//! `System` every frame.
//!
//! Left/Right switch between the three views. Everything is drawn with
//! filled rectangles (horizontal runs of one colour), which is slow by
//! rendering standards but well under a frame on any machine.
//!
//! - Patterns: both 128x128 tables at 2x, coloured with one of the eight
//!   palettes (Up/Down or 0-7 pick it). CHR comes through the mapper's
//!   current banking via `System::peek_chr`.
//! - Nametables: the four tables as a 2x2 grid at half scale, resolved
//!   through the cartridge's current mirroring and coloured through the
//!   attribute bytes, using the background pattern table PPUCTRL selects.
//! - Palettes: the 32 palette RAM entries as swatches with hex values.

use crate::ui::key::Key;

use nes_emu::cartridge::Mirroring;
use nes_emu::ppu::{PpuCtrl, NES_PALETTE};
use nes_emu::system::System;

use crate::ui::app::App;
use crate::ui::font;
use crate::ui::painter::{Color, Painter};
use crate::ui::tool::{self, Tool, ToolEvent};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Patterns,
    Nametables,
    Palettes,
}

impl View {
    const ALL: [View; 3] = [View::Patterns, View::Nametables, View::Palettes];

    fn next(self) -> View {
        let i = View::ALL.iter().position(|&v| v == self).unwrap_or(0);
        View::ALL[(i + 1) % View::ALL.len()]
    }

    fn prev(self) -> View {
        let i = View::ALL.iter().position(|&v| v == self).unwrap_or(0);
        View::ALL[(i + View::ALL.len() - 1) % View::ALL.len()]
    }

    fn name(self) -> &'static str {
        match self {
            View::Patterns => "Patterns",
            View::Nametables => "Nametables",
            View::Palettes => "Palettes",
        }
    }
}

pub struct PpuView {
    view: View,
    /// Palette (0-3 background, 4-7 sprite) used to colour the pattern
    /// tables.
    palette: u8,
}

impl Default for PpuView {
    fn default() -> Self {
        PpuView {
            view: View::Patterns,
            palette: 0,
        }
    }
}

/// A small RGB image built pixel by pixel, then blitted as runs.
struct Image {
    width: usize,
    height: usize,
    pixels: Vec<(u8, u8, u8)>,
}

impl Image {
    fn new(width: usize, height: usize) -> Self {
        Image {
            width,
            height,
            pixels: vec![(0, 0, 0); width * height],
        }
    }

    fn set(&mut self, x: usize, y: usize, colour: (u8, u8, u8)) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = colour;
        }
    }

    /// Draw at (`x`, `y`) with each pixel `scale` window pixels square,
    /// merging horizontal runs of equal colour into one rectangle.
    fn blit(&self, painter: &mut dyn Painter, x: i32, y: i32, scale: u32) -> Result<(), String> {
        for row in 0..self.height {
            let line = &self.pixels[row * self.width..(row + 1) * self.width];
            let mut start = 0;
            while start < self.width {
                let colour = line[start];
                let mut end = start + 1;
                while end < self.width && line[end] == colour {
                    end += 1;
                }
                painter.fill_rect(
                    x + (start as u32 * scale) as i32,
                    y + (row as u32 * scale) as i32,
                    (end - start) as u32 * scale,
                    scale,
                    Color::rgb(colour.0, colour.1, colour.2),
                )?;
                start = end;
            }
        }
        Ok(())
    }
}

/// RGB for palette RAM entry `index` (0-31). Entry 0 of every
/// background sub-palette shows the universal background colour, as the
/// PPU renders it.
fn palette_colour(system: &System, index: u8) -> (u8, u8, u8) {
    let index = if index & 0x03 == 0 && index < 0x10 {
        0
    } else {
        index & 0x1F
    };
    NES_PALETTE[(system.ppu.palette[index as usize] & 0x3F) as usize]
}

/// The two-bit pixel `x` of row `y` of tile `tile` in pattern table
/// `table` (0 or 1).
fn pattern_pixel(system: &System, table: u16, tile: u16, x: u16, y: u16) -> u8 {
    let base = table * 0x1000 + tile * 16 + y;
    let lo = system.peek_chr(base);
    let hi = system.peek_chr(base + 8);
    let shift = 7 - x;
    ((lo >> shift) & 1) | (((hi >> shift) & 1) << 1)
}

/// Index into `nametable_ram` for nametable `table` (0-3) under
/// `mirroring`; the same mapping the PPU applies to $2000-$2FFF.
pub fn physical_table(table: usize, mirroring: Mirroring) -> usize {
    match mirroring {
        Mirroring::Horizontal => table / 2,
        Mirroring::Vertical => table % 2,
        Mirroring::FourScreen => table,
        Mirroring::SingleScreenLower => 0,
        Mirroring::SingleScreenUpper => 1,
    }
}

fn mirroring_of(system: &System) -> Mirroring {
    system
        .cartridge
        .as_ref()
        .map(|c| c.mapper.mirroring())
        .unwrap_or(Mirroring::Horizontal)
}

fn mirroring_name(m: Mirroring) -> &'static str {
    match m {
        Mirroring::Horizontal => "horizontal",
        Mirroring::Vertical => "vertical",
        Mirroring::FourScreen => "four-screen",
        Mirroring::SingleScreenLower => "single (lower)",
        Mirroring::SingleScreenUpper => "single (upper)",
    }
}

impl PpuView {
    fn draw_patterns(
        &self,
        painter: &mut dyn Painter,
        font_scale: u32,
        app: &App,
        y: i32,
    ) -> Result<(), String> {
        let system = &app.system;
        let x0 = tool::padding(font_scale);
        let step = tool::line_step(font_scale);
        let ctrl = system.ppu.ctrl;
        let info = format!(
            "Palette {} (Up/Down)  BG ${:04X}  SPR ${:04X} {}",
            self.palette,
            if ctrl.contains(PpuCtrl::BG_PATTERN) {
                0x1000
            } else {
                0
            },
            if ctrl.contains(PpuCtrl::SPRITE_PATTERN) {
                0x1000
            } else {
                0
            },
            if ctrl.contains(PpuCtrl::SPRITE_SIZE) {
                "8x16"
            } else {
                "8x8"
            },
        );
        font::draw_text(painter, x0, y, font_scale, tool::TEXT, &info)?;
        let y = y + step;

        let scale = 2;
        let gap = 4 * font_scale as i32;
        let colours: [(u8, u8, u8); 4] =
            std::array::from_fn(|i| palette_colour(system, self.palette * 4 + i as u8));
        for table in 0..2u16 {
            let x = x0 + table as i32 * (128 * scale as i32 + gap);
            font::draw_text(
                painter,
                x,
                y,
                font_scale,
                tool::DIM_TEXT,
                &format!("${:04X}", table * 0x1000),
            )?;
            let mut image = Image::new(128, 128);
            for tile in 0..256u16 {
                let (tx, ty) = ((tile % 16) as usize * 8, (tile / 16) as usize * 8);
                for py in 0..8u16 {
                    for px in 0..8u16 {
                        let bits = pattern_pixel(system, table, tile, px, py);
                        image.set(tx + px as usize, ty + py as usize, colours[bits as usize]);
                    }
                }
            }
            image.blit(painter, x, y + step, scale)?;
        }
        Ok(())
    }

    fn draw_nametables(
        &self,
        painter: &mut dyn Painter,
        font_scale: u32,
        app: &App,
        y: i32,
    ) -> Result<(), String> {
        let system = &app.system;
        let x0 = tool::padding(font_scale);
        let step = tool::line_step(font_scale);
        let mirroring = mirroring_of(system);
        let bg_table = if system.ppu.ctrl.contains(PpuCtrl::BG_PATTERN) {
            1
        } else {
            0
        };
        let info = format!(
            "Mirroring {}  BG ${:04X}  half scale",
            mirroring_name(mirroring),
            bg_table * 0x1000
        );
        font::draw_text(painter, x0, y, font_scale, tool::TEXT, &info)?;
        let y = y + step;

        let scale = 2;
        let gap = 2 * font_scale as i32;
        for table in 0..4usize {
            let physical = physical_table(table, mirroring);
            let ram = &system.ppu.nametable_ram[physical * 0x400..(physical + 1) * 0x400];
            let mut image = Image::new(128, 120);
            for ty in 0..30usize {
                for tx in 0..32usize {
                    let tile = ram[ty * 32 + tx] as u16;
                    let attr = ram[0x3C0 + (ty / 4) * 8 + tx / 4];
                    let shift = ((ty & 2) << 1) | (tx & 2);
                    let sub = (attr >> shift) & 0x03;
                    // Half scale: sample every other pixel of the tile.
                    for sy in 0..4u16 {
                        for sx in 0..4u16 {
                            let bits = pattern_pixel(system, bg_table, tile, sx * 2, sy * 2);
                            let colour = palette_colour(system, sub * 4 + bits);
                            image.set(tx * 4 + sx as usize, ty * 4 + sy as usize, colour);
                        }
                    }
                }
            }
            let gx = x0 + (table % 2) as i32 * (128 * scale as i32 + gap);
            let gy = y + (table / 2) as i32 * (120 * scale as i32 + gap + step);
            let label = format!("${:04X} -> {}", 0x2000 + table * 0x400, physical);
            font::draw_text(painter, gx, gy, font_scale, tool::DIM_TEXT, &label)?;
            image.blit(painter, gx, gy + step, scale)?;
        }
        Ok(())
    }

    fn draw_palettes(
        &self,
        painter: &mut dyn Painter,
        font_scale: u32,
        app: &App,
        y: i32,
    ) -> Result<(), String> {
        let system = &app.system;
        let x0 = tool::padding(font_scale);
        let glyph = font::line_height(font_scale) as i32;
        let swatch = 2 * glyph;
        let row_h = swatch + glyph;
        font::draw_text(
            painter,
            x0,
            y,
            font_scale,
            tool::TEXT,
            "Palette RAM $3F00-$3F1F",
        )?;
        let mut y = y + tool::line_step(font_scale) + glyph / 2;
        for group in 0..8u8 {
            let label = if group < 4 {
                format!("BG{group}")
            } else {
                format!("SP{}", group - 4)
            };
            let text_y = y + (swatch - glyph) / 2;
            font::draw_text(painter, x0, text_y, font_scale, tool::ACCENT, &label)?;
            for entry in 0..4u8 {
                let index = group * 4 + entry;
                let raw = system.ppu.palette[index as usize] & 0x3F;
                let (r, g, b) = palette_colour(system, index);
                let x = x0 + 5 * glyph + entry as i32 * (swatch + 4 * glyph);
                painter.fill_rect(
                    x - 1,
                    y - 1,
                    swatch as u32 + 2,
                    swatch as u32 + 2,
                    tool::DIM_TEXT,
                )?;
                painter.fill_rect(x, y, swatch as u32, swatch as u32, Color::rgb(r, g, b))?;
                font::draw_text(
                    painter,
                    x + swatch + glyph / 2,
                    text_y,
                    font_scale,
                    tool::TEXT,
                    &format!("{raw:02X}"),
                )?;
            }
            y += row_h;
        }
        font::draw_text(
            painter,
            x0,
            y + glyph / 2,
            font_scale,
            tool::DIM_TEXT,
            "Entry 0 of BG1-3 shows the universal colour",
        )
    }
}

impl Tool for PpuView {
    fn title(&self) -> &str {
        "PPU"
    }

    fn handle_key(&mut self, key: Key, _app: &mut App) -> ToolEvent {
        match key {
            Key::Right | Key::Tab => self.view = self.view.next(),
            Key::Left => self.view = self.view.prev(),
            Key::Up => self.palette = (self.palette + 1) % 8,
            Key::Down => self.palette = (self.palette + 7) % 8,
            Key::Char('q') => return ToolEvent::Close,
            _ => {
                if let Some(d) = key.digit().filter(|d| *d <= 7) {
                    self.palette = d;
                }
            }
        }
        ToolEvent::Continue
    }

    fn draw(&self, painter: &mut dyn Painter, font_scale: u32, app: &App) -> Result<(), String> {
        let x = tool::padding(font_scale);
        let y = tool::body_top(font_scale);
        let tabs: Vec<String> = View::ALL
            .iter()
            .map(|v| {
                if *v == self.view {
                    format!("[{}]", v.name())
                } else {
                    v.name().to_string()
                }
            })
            .collect();
        // 42 columns at most, inside the 44 the default window offers.
        let header = format!("{}  Left/Right", tabs.join(" "));
        font::draw_text(painter, x, y, font_scale, tool::ACCENT, &header)?;
        let y = y + tool::line_step(font_scale);
        match self.view {
            View::Patterns => self.draw_patterns(painter, font_scale, app, y),
            View::Nametables => self.draw_nametables(painter, font_scale, app, y),
            View::Palettes => self.draw_palettes(painter, font_scale, app, y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_cycle_both_ways() {
        let mut v = View::Patterns;
        for _ in 0..3 {
            v = v.next();
        }
        assert_eq!(v, View::Patterns);
        assert_eq!(View::Patterns.prev(), View::Palettes);
        assert_eq!(View::Palettes.next(), View::Patterns);
    }

    #[test]
    fn physical_table_follows_mirroring() {
        let h: Vec<usize> = (0..4)
            .map(|t| physical_table(t, Mirroring::Horizontal))
            .collect();
        assert_eq!(h, [0, 0, 1, 1]);
        let v: Vec<usize> = (0..4)
            .map(|t| physical_table(t, Mirroring::Vertical))
            .collect();
        assert_eq!(v, [0, 1, 0, 1]);
        let f: Vec<usize> = (0..4)
            .map(|t| physical_table(t, Mirroring::FourScreen))
            .collect();
        assert_eq!(f, [0, 1, 2, 3]);
        assert_eq!(physical_table(3, Mirroring::SingleScreenLower), 0);
        assert_eq!(physical_table(0, Mirroring::SingleScreenUpper), 1);
    }

    #[test]
    fn pattern_and_palette_lookups_use_the_system() {
        let mut system = System::new();
        let cart = nes_emu::cartridge::Cartridge::load_from_bytes(&nrom_with_chr_ram())
            .expect("synthetic ROM must parse");
        system.load_cartridge(cart);
        // Tile 1 of table 0, row 0: low plane $80, high plane $01.
        system.debug_write(0x2006, 0x00);
        system.debug_write(0x2006, 0x10);
        system.debug_write(0x2007, 0x80);
        system.debug_write(0x2006, 0x00);
        system.debug_write(0x2006, 0x18);
        system.debug_write(0x2007, 0x01);
        assert_eq!(pattern_pixel(&system, 0, 1, 0, 0), 1);
        assert_eq!(pattern_pixel(&system, 0, 1, 7, 0), 2);
        assert_eq!(pattern_pixel(&system, 0, 1, 3, 0), 0);

        system.ppu.palette[0] = 0x0F;
        system.ppu.palette[4] = 0x30;
        system.ppu.palette[5] = 0x16;
        assert_eq!(palette_colour(&system, 4), NES_PALETTE[0x0F]);
        assert_eq!(palette_colour(&system, 5), NES_PALETTE[0x16]);
    }

    /// Mapper 0, 32 KB PRG, no CHR ROM so the loader gives 8 KB CHR RAM.
    fn nrom_with_chr_ram() -> Vec<u8> {
        let mut rom = b"NES\x1A".to_vec();
        rom.extend_from_slice(&[2, 0, 0, 0]);
        rom.extend_from_slice(&[0; 8]);
        rom.extend(std::iter::repeat_n(0xEAu8, 0x8000));
        rom
    }
}
