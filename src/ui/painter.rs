//! Frontend-neutral drawing surface for the overlay UI.
//!
//! Everything the palette, the toasts and the tool pages draw is a
//! sequence of filled rectangles (text is one rectangle per set font
//! pixel, see `font::draw_text`). [`Painter`] is that one operation plus
//! the surface size, so the same drawing code runs on the SDL window
//! (`SdlPainter` in `main.rs`, which forwards to `WindowCanvas::fill_rect`)
//! and on an RGBA buffer the web page composites over the game
//! ([`RgbaPainter`]). Keeping the interface this small is what keeps the
//! SDL screenshots pixel-identical across backends: there is nothing to
//! reinterpret.

/// An RGBA colour; `a` is straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }
}

/// A surface the UI draws on.
pub trait Painter {
    /// Width and height of the surface in pixels.
    fn size(&self) -> (u32, u32);

    /// Fill the rectangle with `colour`, blending translucent colours
    /// over what is there. Parts outside the surface are clipped.
    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, colour: Color) -> Result<(), String>;
}

/// An in-memory RGBA surface with source-over blending, transparent
/// until drawn on. The web page uploads [`RgbaPainter::pixels`] to a
/// canvas layered over the game frame.
// Unused by the SDL binary; the web frontend constructs it once this
// module lives in the library (docs/plans/SHARED_OVERLAY_UI.md, Phase 3).
#[allow(dead_code)]
pub struct RgbaPainter {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[allow(dead_code)]
impl RgbaPainter {
    pub fn new(width: u32, height: u32) -> Self {
        RgbaPainter {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    /// Change the surface size; the contents are cleared.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels = vec![0; (width * height * 4) as usize];
    }

    /// Make every pixel transparent black.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// The surface, row-major RGBA, `width * height * 4` bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel at (`x`, `y`) as `[r, g, b, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

/// Source-over composite of straight-alpha `src` onto `dst`, in place.
/// Integer arithmetic with rounding; an opaque source replaces, a fully
/// transparent one leaves `dst` alone, and any source over a transparent
/// destination yields the source unchanged (including its alpha).
#[allow(dead_code)]
fn blend(dst: &mut [u8], src: Color) {
    let sa = src.a as u32;
    if sa == 255 {
        dst.copy_from_slice(&[src.r, src.g, src.b, 255]);
        return;
    }
    if sa == 0 {
        return;
    }
    let da = dst[3] as u32;
    // Alpha and colours scaled by 255 to stay in integers.
    let out_a = sa * 255 + da * (255 - sa);
    if out_a == 0 {
        return;
    }
    let channel = |s: u8, d: u8| -> u8 {
        let num = s as u32 * sa * 255 + d as u32 * da * (255 - sa);
        ((num + out_a / 2) / out_a) as u8
    };
    dst[0] = channel(src.r, dst[0]);
    dst[1] = channel(src.g, dst[1]);
    dst[2] = channel(src.b, dst[2]);
    dst[3] = ((out_a + 127) / 255) as u8;
}

impl Painter for RgbaPainter {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, colour: Color) -> Result<(), String> {
        let x0 = x.max(0) as i64;
        let y0 = y.max(0) as i64;
        let x1 = (x as i64 + w as i64).min(self.width as i64);
        let y1 = (y as i64 + h as i64).min(self.height as i64);
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let start = row as usize * stride + x0 as usize * 4;
            let end = row as usize * stride + x1 as usize * 4;
            for px in self.pixels[start..end].chunks_exact_mut(4) {
                blend(px, colour);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_fill_sets_pixels_inside_and_leaves_the_rest() {
        let mut p = RgbaPainter::new(8, 4);
        p.fill_rect(2, 1, 3, 2, Color::rgb(10, 20, 30)).unwrap();
        assert_eq!(p.pixel(2, 1), [10, 20, 30, 255]);
        assert_eq!(p.pixel(4, 2), [10, 20, 30, 255]);
        assert_eq!(p.pixel(5, 2), [0, 0, 0, 0]);
        assert_eq!(p.pixel(1, 1), [0, 0, 0, 0]);
        assert_eq!(p.pixel(2, 3), [0, 0, 0, 0]);
        assert_eq!(p.size(), (8, 4));
        assert_eq!(p.pixels().len(), 8 * 4 * 4);
    }

    #[test]
    fn translucent_over_transparent_keeps_the_source_alpha() {
        let mut p = RgbaPainter::new(2, 2);
        p.fill_rect(0, 0, 2, 2, Color::rgba(8, 10, 24, 236))
            .unwrap();
        assert_eq!(p.pixel(0, 0), [8, 10, 24, 236]);
        assert_eq!(p.pixel(1, 1), [8, 10, 24, 236]);
    }

    #[test]
    fn translucent_over_opaque_blends_and_stays_opaque() {
        let mut p = RgbaPainter::new(1, 1);
        p.fill_rect(0, 0, 1, 1, Color::rgb(200, 100, 0)).unwrap();
        p.fill_rect(0, 0, 1, 1, Color::rgba(0, 0, 0, 180)).unwrap();
        // 200 * 75 / 255 = 58.8, 100 * 75 / 255 = 29.4
        assert_eq!(p.pixel(0, 0), [59, 29, 0, 255]);
        // A fully transparent source changes nothing.
        p.fill_rect(0, 0, 1, 1, Color::rgba(255, 255, 255, 0))
            .unwrap();
        assert_eq!(p.pixel(0, 0), [59, 29, 0, 255]);
    }

    #[test]
    fn out_of_bounds_is_clipped_or_ignored() {
        let mut p = RgbaPainter::new(4, 4);
        p.fill_rect(-2, -2, 3, 3, Color::rgb(1, 2, 3)).unwrap();
        assert_eq!(p.pixel(0, 0), [1, 2, 3, 255]);
        assert_eq!(p.pixel(1, 1), [0, 0, 0, 0]);
        p.fill_rect(3, 3, 10, 10, Color::rgb(4, 5, 6)).unwrap();
        assert_eq!(p.pixel(3, 3), [4, 5, 6, 255]);
        p.fill_rect(10, 10, 2, 2, Color::rgb(7, 8, 9)).unwrap();
        p.fill_rect(-10, 0, 2, 2, Color::rgb(7, 8, 9)).unwrap();
        p.fill_rect(0, 0, 0, 5, Color::rgb(7, 8, 9)).unwrap();
        assert!(p.pixels().iter().all(|&b| b != 7));
    }

    #[test]
    fn clear_and_resize_make_everything_transparent() {
        let mut p = RgbaPainter::new(2, 2);
        p.fill_rect(0, 0, 2, 2, Color::rgb(9, 9, 9)).unwrap();
        p.clear();
        assert!(p.pixels().iter().all(|&b| b == 0));
        p.fill_rect(0, 0, 2, 2, Color::rgb(9, 9, 9)).unwrap();
        p.resize(3, 1);
        assert_eq!(p.size(), (3, 1));
        assert_eq!(p.pixels().len(), 12);
        assert!(p.pixels().iter().all(|&b| b == 0));
    }
}
