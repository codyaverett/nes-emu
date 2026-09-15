# Widescreen Background Extension

**Date:** 2026-09-14
**Type:** Feature Plan
**Status:** Phases 1 to 4 implemented on branch widescreen (docs/debugging/WIDESCREEN.md); the panorama pixel cache from the Phase 4 sketch was dropped after measurement, the stale-column rule uses $2007 write stamps instead
**Tracking:** parent #63; Phase 1 #64, Phase 2 #65, Phase 3 #66, Phase 4 #67

## Goal

An optional wide view that shows background tiles beyond the left and
right edges of the 256-pixel picture, drawn from the same nametable
memory the game already fills, toggled on and off at any moment while
playing (palette command and hotkey) on both the SDL binary and the web
page. Off by default, and with the feature off every existing output
stays byte-identical.

## Why it can work

Horizontally scrolling games on vertical-mirroring boards keep a
512-pixel strip of tiles in VRAM and display a 256-pixel window of it.
A probe run headlessly against this emulator (2026-09-14) measured how
often the columns beyond each edge already held the tiles later shown
there:

| Game | Right side valid at 59 px | at 104 px | Left side intact at 136 px |
| --- | --- | --- | --- |
| Super Mario Bros | 99.8% | 95% | 100%, gone by 160 px |
| Contra | 96% | 94% | 77% |
| River City Ransom | about 30% | about 30% | 73% |

A 16:9 picture at 240 lines with the NES 8:7 pixel aspect is 373 NES
pixels wide, so 59 extra per side; at the 224-line overscan crop it is
348, so 46 per side. SMB and Contra cover that on both sides. River
City Ransom draws columns just in time, so its right side needs the
fallback in Phase 4.

## What cannot be extended

- Sprites. Sprite X is one byte and games spawn enemies at the picture
  edge, so sprites stay inside the native 256 columns and enemies pop in
  at the old edge. This is inherent and is documented, not fixed.
- HUD splits. SMB and River City Ransom change the scroll mid-frame for a
  status bar, so the extension must use the scroll each scanline actually
  rendered with, never a per-frame value.
- The 512-pixel strip. Tiles behind the window survive only until the
  game reuses that memory for new columns.

## Architecture

```
Ppu::step
  end of dot 320, lines 261 and 0..238   -> capture (v, x, ctrl) for the next line
  when wide is on: render_wide_line(N)  -> left/right strips into wide_buffer
  end of frame                          -> centre 256 columns copied from frame_buffer
System::get_wide_frame_buffer()         -> 384x240 RGB (64 px each side, max)
App { wide: WideMode, wide_dirty }      -> palette `wide`, hotkey W, toast
SDL: window width = (256 + 2*ext) * 3, texture 384 wide, src rect crops to ext
Web: canvas width and CSS aspect derived from a core `visible_size`, not literals
Overlay UI: unchanged 720x672 grid, centred over the wide frame by an x offset
```

`WideMode` is `Off`, `Wide16x9` (ext computed from the visible height and
the 8:7 pixel aspect: 59 at 240 lines, 46 at 224) and `Max` (64 per
side). The renderer always fills 64 per side when any mode is on; the
frontends crop to the mode's extension, so switching modes is a resize
with no PPU change.

The side strips are background only: nametable byte via the mapper's
current mirroring, attribute byte, pattern bytes through the mapper's
side-effect-free `ppu_peek` (so MMC3 A12 counting is untouched), palette
through the same lookup as the centre, honouring the show-background bit
and the left-edge mask bit for the left strip.

The scroll capture is derived render state like the frame buffer: it is
not written to save states, so the state format version does not move.

## Constraints

- Feature off: `frame_buffer`, every screenshot in `docs/testing/test_output/ui/`,
  `tests/game_frames.rs` fingerprints and save states are byte-identical.
- No new crates. `cargo build --no-default-features` and the wasm build pass.
- Zero cost when off: the per-line scroll capture is a few stores; the
  strip renderer runs only when a wide mode is on.
- Gates per phase: build, test, clippy -D warnings, fmt, `npm test` for
  web phases.

## Phases

### Phase 1: PPU scroll capture and wide renderer

- `ScanlineScroll { v, x, ctrl, mask }` captured per line at the end of
  dot 320 of the previous line (after `copy_x`, before the prefetch
  `increment_x`).
- `Ppu::wide_enabled: bool`, `wide_buffer: Box<[u8; 384*240*3]>`,
  `render_wide_line`, `get_wide_frame_buffer`; `System` passthroughs and
  a `set_wide_enabled`.
- Tests: a synthetic ROM (built with the nes-rom-creator toolchain or a
  hand-assembled PRG) that fills both nametables with a known pattern and
  scrolls, asserting each strip pixel equals the tile expected from the
  scroll; a mid-frame split case; the SMB probe as an integration test
  (`#[ignore]`, needs roms/) asserting the right strip matches the later
  visible columns at least 95% of scrolling frames at 59 px.

### Phase 2: SDL toggle, palette command, hotkey

- `App::wide`, `toggle_wide` (cycles Off, 16:9, Max), `wide off|16:9|max`
  palette command, hotkey W, toast, `wide_dirty` handled like `crop_dirty`:
  window and texture resize, source rect crops to the mode's extension
  and the overscan crop.
- Overlay UI centred with an x offset on the SDL painter; `--screenshot`
  captures the wide window; `scripts/ui_screenshots.sh` unchanged and
  still byte-identical with the feature off.
- Help page lists the key and command.

### Phase 3: Web page

- `visible_size()` from the core replaces the literal 240/256 widths in
  `web/app.js` and `web/index.html`; canvas, overlay canvas and CSS
  aspect ratio follow it; `frame_rgba` returns the wide buffer when on.
- The palette command and W key already route through the shared UI.

### Phase 4: Invalid-column fallback and per-game profiles

- Panorama fallback: remember the last tiles seen at each world column
  (scroll accumulated per frame) and draw them where the live strip
  column differs from what was last shown at that world position; masks
  the stale data games like River City Ransom leave to the right.
- Per-game profile keyed by ROM CRC-32 in `cheats/`-style data files:
  left and right allowance in pixels, HUD rows to draw at native width.
- Settings persistence (mode survives restart) if a settings store is
  added; out of scope until then.

## Verification

Headless: the tests above. Visual: `--screenshot` of SMB 1-1 at frame
400 with `wide max`, checked by a human once per phase and kept under
`docs/testing/test_output/wide/`. Manual: toggle W repeatedly during
play; no frame drop, no stale overlay geometry.
