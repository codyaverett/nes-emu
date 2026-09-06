# Shared Overlay UI: Painter, Key and Host (issue #52)

**Date:** 2026-09-06
**Type:** Refactor Plan
**Status:** Implemented (#58, #59, #60, #61 on branch issue-52-shared-ui; docs/debugging/SHARED_OVERLAY_UI.md); Phase 6 CHANGELOG and version bump happen at merge
**Tracking:** GitHub issue #52 (Phase 4 of `docs/plans/WASM_WEB.md`) and the sub-issues listed per phase

## Goal

Move `src/ui/` into the `nes_emu` library behind three frontend-neutral
abstractions so the command palette, the six tool pages (Help, Cheats,
Memory, PPU, APU, States), the toasts and rewind run identically on the
SDL binary and the web page from one code path.

## Constraints

- SDL screenshots stay pixel-identical: every draw remains the same
  sequence of colour plus rectangle fills; `font::draw_text` stays the
  only text implementation (one rect per set pixel) on both backends.
- `cargo build --no-default-features` and the wasm build must pass from
  Phase 3 on: nothing under `src/ui/` may reach `Instant`, `SystemTime`
  or `std::fs` on wasm.
- No new crates. `--ui-script` and `--screenshot` keep their CLI and the
  `Keycode::from_name` parsing.
- While the palette or a page is open every key is consumed (an
  unmapped key must not leak to the controller).
- Gates each phase: build, test, clippy -D warnings, fmt, `npm test`.

## Abstractions

```rust
// src/ui/painter.rs
pub struct Color { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }   // const fn rgb / rgba
pub trait Painter {
    fn size(&self) -> (u32, u32);
    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, colour: Color) -> Result<(), String>;
}
pub struct RgbaPainter { .. }   // src-over blending, clipping, clear(), pixels()

// src/ui/key.rs
pub enum Key { Char(char), Backquote, Escape, Return, Backspace, Delete, Insert,
               Up, Down, Left, Right, PageUp, PageDown, Home, End, F1, .., F8, Other }
impl Key { pub fn printable(self) -> Option<char>; pub fn from_browser_code(code: &str) -> Key }
// Space is Char(' '); Backquote is never printable; KpEnter folds into Return.

// src/ui/host.rs
pub struct SlotInfo { pub modified_unix_secs: Option<u64>, pub size: u64 }
pub trait Host {
    fn write_state(&mut self, slot: u8, image: &[u8]) -> Result<(), String>;
    fn read_state(&mut self, slot: u8) -> Result<Option<Vec<u8>>, String>;
    fn slot_info(&self, slot: u8) -> Option<SlotInfo>;
    fn slot_label(&self, slot: u8) -> String;
    fn write_cheats(&mut self, text: &str) -> Result<(), String>;
    fn cheats_label(&self) -> String;
}
#[cfg(not(target_arch = "wasm32"))] pub struct FileHost { rom_path: PathBuf }   // <rom>.sN, <rom>.cht
```

`App` gets `host: Box<dyn Host>` and a `now_ms: u64` clock the frontend
sets before drawing; toast expiry uses it instead of `Instant`.
`WINDOW_SCALE = 3` moves into `src/ui/mod.rs`. `draw_messages` and the
shared hotkeys (R, P, N, M, +, -, F5-F8, Backspace) move into the
library so both frontends call one function.

## Phases

### Phase 0: baseline harness
`scripts/ui_screenshots.sh` runs the `--screenshot`/`--ui-script`
commands from `docs/debugging/UI_FRAMEWORK.md` with `--no-audio` for
every page and the palette, using `roms/mario.nes`, with `<rom>.s1..s3`
pre-created at a fixed mtime so the States page is stable, and prints
`shasum -a 256` of the PPMs. Baseline recorded on the untouched tree;
two consecutive runs must agree.

### Phase 1: Painter (draw side), still inside the binary
`painter.rs` with unit tests (opaque fill, translucent over transparent
yields the source alpha, out-of-bounds is a no-op); `font.rs`,
`tool.rs`, `palette.rs`, every tool and `Ui::draw` take `&mut dyn
Painter`; `SdlPainter` in `main.rs` wraps the `WindowCanvas`. Hashes
equal the baseline.

### Phase 2: Key (input side)
`key.rs` with browser-code mapping tests; `Keycode` leaves `src/ui/`;
`main.rs` maps SDL keys to `Key` (printable 32..=126 to `Char`, named
keys, else `Other`) and routes every key through the `Ui` first.
Existing palette and cheats key tests converted. Hashes unchanged.

### Phase 3: Host and clock, then the move into the library
`host.rs` with `FileHost`; `App::new(system, audio, muted, volume,
crop, host)`; slot and cheat file IO go through the host; `now_ms`
replaces `Instant`; `draw_messages` and `hotkey` move in; then `pub mod
ui;` in `src/lib.rs` and `use nes_emu::ui;` in `main.rs`. Hashes
unchanged; `--no-default-features` and the wasm32 build pass.

### Phase 4: wasm wrapper
`web/src/host.rs` `WebHost` (nine in-memory slots plus dirty flags the
page syncs with IndexedDB); `Emulator` owns `App`, `Ui` and an
`RgbaPainter`; new exports `key_down(code) -> bool`, `key_up(code)`,
`set_now_ms`, `overlay_size`, `overlay_visible`, `overlay_rgba`,
`paused/muted/volume/crop_enabled/slot/rewinding/ui_active`,
`set_paused`, `take_frame_advance`, `rewind_step`, `set_slot_cache`,
`take_dirty_slots`, `slot_bytes`, `take_cheats_dirty`, `osd_message`.
Smoke test covers palette open, overlay size and alpha, rewind and a
dirty slot after F5.

### Phase 5: page
Overlay canvas over the frame; keys go through `key_down` before the
controller table (R, P, M and F5-F8 handled by the library; F stays in
JS for fullscreen); per tick `set_now_ms`, rewind branch, sync of
pause, mute, volume, crop and slot into the existing controls, slot and
cheat persistence from the dirty flags. Toasts draw through the shared
overlay (one look, one code path); the HTML toast is removed.

### Phase 6: docs and release
`docs/debugging/UI_FRAMEWORK.md` architecture and harness sections,
`docs/plans/WASM_WEB.md` Phase 4 status, CHANGELOG, version 0.15.0.

## Risks

- Toast expiry now evaluated at `now_ms` set before the draw instead of
  at `Instant::now()`; the harness after Phases 1-3 detects drift.
- Never regenerate the committed PNGs under `docs/testing/test_output/ui/`
  in this work; comparison is against the Phase 0 baseline only.
- `RgbaPainter` blending or clipping errors (the PPU page draws swatches
  at x-1); the Phase 1 unit tests catch them before any wasm work.
- The web slot cache is empty until the IndexedDB read resolves; an F8
  in the first frames reports an empty slot. Documented, not fixed.
- Rewind on the web holds about 9 MB of states and `overlay_rgba`
  copies about 1.9 MB per frame while visible; watch the stats line.
