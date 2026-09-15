//! Wide view fallback: decide per strip column whether the live nametable
//! data is trustworthy (docs/plans/WIDESCREEN.md, Phase 4).
//!
//! Games fill the 512-pixel nametable strip lazily: to the right of the
//! picture the tiles are valid only as far ahead as the game has written.
//! Positions are absolute strip coordinates, the strip's own 8-pixel tile
//! grid extended without wrapping (starting from the first frame's scroll
//! and following scroll deltas), so a tile index modulo 64 is the strip
//! column it lives in and the previous occupant of that memory is the
//! column 64 back. A strip column is:
//!
//! - **live** when it has been visible at this absolute position before,
//!   or the game has written its memory since the previous occupant was
//!   last visible;
//! - **stale** when it has never been visible and nothing has been written
//!   there since the previous occupant was shown: what is there is that
//!   occupant's leftovers, so the backdrop is drawn instead.
//!
//! Only writes decide. Comparing tiles against the previous occupant is
//! wrong because level columns repeat (every all-sky column is identical),
//! and remembering pixels for columns the game redraws is wrong because
//! games redraw scenes in place without moving the scroll. Live memory is
//! the better answer in both cases.

use super::{LineScroll, SCREEN_WIDTH};

/// Tile rows in a nametable.
pub const ROWS: usize = 30;
/// Absolute tile columns remembered (a ring): 4096 pixels of level.
const TILE_RING: usize = 512;
/// A scroll jump larger than this between frames is a scene change: the
/// history is dropped.
const MAX_DELTA: i32 = 128;

/// Per-game adjustments, matched by ROM CRC-32 (docs/debugging/WIDESCREEN.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WideProfile {
    /// Lines at the top that are a status bar: strips draw the backdrop.
    pub hud_top: u16,
    /// Lines at the bottom that are a status bar.
    pub hud_bottom: u16,
}

impl WideProfile {
    /// Built-in profiles. CRC is of the image after the iNES header, the
    /// same value the cheat database uses.
    pub fn for_crc(crc: u32) -> WideProfile {
        match crc {
            // Super Mario Bros (World): status bar in rows 0-3.
            0x8E2B_D25C | 0xD26E_FD78 => WideProfile {
                hud_top: 32,
                hud_bottom: 0,
            },
            _ => WideProfile::default(),
        }
    }
}

/// What to draw for one strip tile column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Draw the live nametable data.
    Live,
    /// Draw the backdrop colour.
    Stale,
}

pub struct WideCache {
    /// Frames fed to `end_of_frame` so far; stamps below are in this unit.
    frame: u64,
    /// Absolute strip x of the picture's left edge at the end of the last
    /// frame.
    world: i64,
    /// Scroll x (0-511) of the last frame's main line, for the delta.
    prev_scroll: Option<u16>,
    /// Scroll x of the main line in the last frame; per-line positions
    /// are relative to it.
    main_scroll: u16,
    /// Absolute tile column each ring slot last held, and the frame it was
    /// last visible in.
    seen_col: Vec<i64>,
    seen_frame: Vec<u64>,
    /// Frame stamp of the last `$2007` write into each of the 64 strip
    /// columns (playfield rows only).
    written_frame: Vec<u64>,
}

impl Default for WideCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WideCache {
    pub fn new() -> Self {
        WideCache {
            frame: 0,
            world: 0,
            prev_scroll: None,
            main_scroll: 0,
            seen_col: vec![i64::MIN; TILE_RING],
            seen_frame: vec![0; TILE_RING],
            written_frame: vec![0; 64],
        }
    }

    /// Forget the history (scene change).
    pub fn clear(&mut self) {
        self.seen_col.iter_mut().for_each(|w| *w = i64::MIN);
        self.prev_scroll = None;
    }

    /// Absolute strip x of the picture's left edge.
    pub fn world(&self) -> i64 {
        self.world
    }

    /// A `$2007` write landed in strip column `strip_col` (0-63) on a
    /// playfield row. Writes happen in vblank, after `end_of_frame` for the
    /// picture just shown, so they are stamped one frame later than the
    /// columns that picture marked as seen.
    pub fn note_write(&mut self, strip_col: u16) {
        self.written_frame[(strip_col & 63) as usize] = self.frame + 1;
    }

    /// Horizontal scroll (0-511) a line rendered with.
    pub fn scroll_x(ls: &LineScroll) -> u16 {
        ((ls.v >> 10) & 1) * 256 + (ls.v & 0x1F) * 8 + ls.x as u16
    }

    /// The scroll most lines used: the playfield's, not a status bar's.
    fn main_scroll(lines: &[LineScroll]) -> Option<u16> {
        let mut counts: Vec<(u16, usize)> = Vec::new();
        for ls in lines.iter().filter(|l| l.captured && l.show_bg) {
            let sx = Self::scroll_x(ls);
            match counts.iter_mut().find(|(s, _)| *s == sx) {
                Some((_, n)) => *n += 1,
                None => counts.push((sx, 1)),
            }
        }
        counts.iter().max_by_key(|(_, n)| *n).map(|(s, _)| *s)
    }

    /// Absolute x of the left picture edge for a line, from its own scroll
    /// relative to the frame's main scroll (status bars differ).
    pub fn line_world(&self, ls: &LineScroll) -> i64 {
        let d = Self::scroll_x(ls) as i32 - self.main_scroll as i32;
        let d = if d > 256 {
            d - 512
        } else if d < -256 {
            d + 512
        } else {
            d
        };
        self.world + d as i64
    }

    /// Strip column (0-63) that absolute tile column `tw` lives in.
    pub fn strip_col(tw: i64) -> u16 {
        tw.rem_euclid(64) as u16
    }

    /// Called once per frame after the picture is complete: track the
    /// scroll and mark the visible tile columns as seen.
    pub fn end_of_frame(&mut self, lines: &[LineScroll]) {
        let Some(scroll) = Self::main_scroll(lines) else {
            return;
        };
        self.frame += 1;
        if let Some(prev) = self.prev_scroll {
            let mut delta = scroll as i32 - prev as i32;
            if delta > 256 {
                delta -= 512;
            } else if delta < -256 {
                delta += 512;
            }
            if delta.abs() > MAX_DELTA {
                self.clear();
                self.world = scroll as i64;
            } else {
                self.world += delta as i64;
            }
        } else {
            self.world = scroll as i64;
        }
        self.prev_scroll = Some(scroll);
        self.main_scroll = scroll;

        let first = (self.world + 7).div_euclid(8);
        let last = (self.world + SCREEN_WIDTH as i64).div_euclid(8); // exclusive
        for tw in first..last {
            let slot = tw.rem_euclid(TILE_RING as i64) as usize;
            self.seen_col[slot] = tw;
            self.seen_frame[slot] = self.frame;
        }
    }

    /// Classify absolute tile column `tw`.
    pub fn decide(&self, tw: i64) -> Decision {
        let slot = tw.rem_euclid(TILE_RING as i64) as usize;
        if self.seen_col[slot] == tw {
            return Decision::Live;
        }
        // Never seen. If the previous occupant of this strip memory was
        // shown and the game has not written the memory since, what is
        // there is its leftovers.
        let prev = tw - 64;
        let pslot = prev.rem_euclid(TILE_RING as i64) as usize;
        let strip_col = Self::strip_col(tw) as usize;
        if self.seen_col[pslot] == prev && self.written_frame[strip_col] <= self.seen_frame[pslot] {
            Decision::Stale
        } else {
            Decision::Live
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppu::SCREEN_HEIGHT;

    fn line(scroll: u16) -> LineScroll {
        LineScroll {
            v: ((scroll / 256) << 10) | ((scroll % 256) / 8),
            x: (scroll % 8) as u8,
            bg_table_hi: false,
            show_bg: true,
            show_bg_left: true,
            captured: true,
        }
    }

    fn lines(scroll: u16) -> Vec<LineScroll> {
        vec![line(scroll); SCREEN_HEIGHT]
    }

    #[test]
    fn scroll_x_round_trips() {
        for s in [0u16, 7, 8, 255, 256, 300, 511] {
            assert_eq!(WideCache::scroll_x(&line(s)), s);
        }
    }

    #[test]
    fn main_scroll_ignores_the_status_bar_minority() {
        let mut ls = lines(100);
        for l in ls.iter_mut().take(32) {
            *l = line(0);
        }
        assert_eq!(WideCache::main_scroll(&ls), Some(100));
    }

    #[test]
    fn world_starts_at_the_first_scroll_follows_deltas_and_resets_on_jumps() {
        let mut c = WideCache::new();
        c.end_of_frame(&lines(239));
        assert_eq!(c.world(), 239);
        c.end_of_frame(&lines(249));
        c.end_of_frame(&lines(227)); // -22
        assert_eq!(c.world(), 227);
        c.end_of_frame(&lines(500)); // wrapped: -239... no, +273 wraps to -239
        assert_eq!(
            c.world(),
            500,
            "a jump over MAX_DELTA restarts at the scroll"
        );
        c.end_of_frame(&lines(10)); // 500 -> 10 is +22 across the wrap
        assert_eq!(c.world(), 522);
        assert_eq!(
            c.line_world(&line(0)),
            512,
            "a line 10 px behind the main scroll"
        );
    }

    #[test]
    fn strip_col_is_the_tile_index_mod_64() {
        assert_eq!(WideCache::strip_col(30), 30);
        assert_eq!(WideCache::strip_col(64 + 30), 30);
        assert_eq!(WideCache::strip_col(-1), 63);
    }

    #[test]
    fn stale_needs_a_shown_previous_occupant_and_no_write_since() {
        let mut c = WideCache::new();
        c.end_of_frame(&lines(0)); // absolute columns 0-31 seen in frame 1
        assert_eq!(c.decide(5), Decision::Live, "seen");
        assert_eq!(
            c.decide(40),
            Decision::Live,
            "never seen, no previous occupant"
        );
        // Column 69 shares memory with column 5. Nothing written: stale.
        assert_eq!(c.decide(69), Decision::Stale);
        // The game writes that memory in vblank: live from then on.
        c.note_write(5);
        assert_eq!(c.decide(69), Decision::Live);
        // But column 5 is shown again in the next frame, so that write was
        // column 5's own (a redraw in place), and 69 is stale once more.
        c.end_of_frame(&lines(0));
        assert_eq!(c.decide(69), Decision::Stale);
        // Scroll on until column 5 is off screen, then a write is 69's.
        c.end_of_frame(&lines(60)); // columns 8-39 visible
        assert_eq!(c.decide(69), Decision::Stale);
        c.note_write(5);
        assert_eq!(c.decide(69), Decision::Live);
        c.end_of_frame(&lines(70));
        assert_eq!(
            c.decide(69),
            Decision::Live,
            "the write is newer than the last sighting"
        );
    }

    #[test]
    fn a_scene_change_forgets_history() {
        let mut c = WideCache::new();
        c.end_of_frame(&lines(0));
        assert_eq!(c.decide(69), Decision::Stale);
        c.end_of_frame(&lines(300));
        assert_eq!(c.decide(69), Decision::Live, "no history after the jump");
    }
}
