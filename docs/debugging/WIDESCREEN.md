# Widescreen: implementation notes (issue #63)

Plan: docs/plans/WIDESCREEN.md.

## Phase 1: scroll capture and strip renderer (#64)

- `LineScroll` is sampled at dot 320 of the previous line. Sampling at
  dot 0 of the line itself is wrong: the prefetch at dots 328 and 336
  has already advanced `v` by two tiles.
- The strips read pattern bytes through `Mapper::ppu_peek`, never
  `ppu_read`, so MMC2/MMC4 latches and the MMC3 A12 counter are not
  disturbed by the extra fetches.
- `wide_buffer` is a `Vec` allocated on first enable; `Ppu` is built by
  value and the wasm stack is already sized for it.
- Test warm-up: the first frame after PPUMASK enables rendering mid-frame
  is partial (no pre-render prefetch), so comparisons use the second
  frame onwards.
- Measured on Super Mario Bros with `tests/widescreen.rs` (ignored, needs
  `roms/`): the right strip matches the picture that scrolls in 8 to 59
  px later on 94.6% of compared pixels over 1011 frame pairs; the
  remainder is sprites present in the later picture.
- Probe results for other games (nametable-level, before the renderer
  existed): Contra 96% valid at 59 px right; River City Ransom about 30%
  (draws columns just in time), motivating the Phase 4 fallback.

## Phase 2: SDL toggle (#65)

- `WideMode` lives on `App`; W and the `wide` palette command cycle
  off, 16:9, max. `wide_dirty` is handled next to `crop_dirty`.
- The SDL texture is always `WIDE_WIDTH` wide. With the view off only the
  picture is uploaded into the middle and the source rectangle is offset
  by `WIDE_EXT`; the 15 UI screenshot hashes stayed identical except the
  two help pages, which now list W.
- In wide mode the overscan crop applies vertically only: the picture's
  edge columns sit in the middle of the image, so cropping them would
  cut a hole. Games that mask the leftmost 8 columns would leave a dark
  bar there; those columns are redrawn from the nametable at dot 320.
- Reference capture: `docs/testing/test_output/wide/smb_title_16x9.png`
  (SMB title, 16:9, window 1050x672). The right strip shows a question
  block outside the native picture.
- Environment: Homebrew `sdl2` is now sdl2-compat over SDL3. The debug
  binary once aborted in the sdl2 crate on event type 0x207 (an SDL3
  window event); the release binary did not. Not caused by this work.

## Phase 3: web page (#66)

- The wasm `Emulator` presents the core's geometry, not literals:
  `frame_width()` (256, or 384 while wide is on), `frame_height()`,
  `picture_rect()` as `[x, y, w, h]` from `App::picture_rect`,
  `wide_mode()` (the label) and `toggle_wide()` for the button.
  `frame_rgba()` converts `get_wide_frame_buffer` (384x240) when a wide
  mode is on and the picture otherwise; `self.rgba` is reallocated when
  the width changes.
- `take_layout_dirty()` is true once after construction and after every
  crop or wide change. It runs the pending overlay resize itself, so the
  page reads consistent `overlay_size` and `picture_rect` whether or not
  `tick` ran first. `syncControls` polls it once per tick and calls
  `applyLayout`, which sizes the `ImageData` at frame size, the canvas
  at the picture rect and draws with `putImageData(image, -x, -y)`.
  `present` also resizes if the RGBA length and the `ImageData` ever
  disagree, so a missed flag cannot throw a RangeError.
- `#screen-wrap` takes `--pw`/`--ph` custom properties (set from the
  picture rect) for both `aspect-ratio` and the viewport-fit `width`
  rule; the `.full-frame` class is gone. A "Wide" button next to "Full
  frame" sends `KeyW` through the page's `keyDown`, the same path as the
  keyboard, so touch users can cycle the modes.
- Verified headless by `web/test/smoke.mjs` (Node build) and the crate's
  unit tests: with the default crop, W gives 16:9 with rect
  `[17, 8, 350, 224]`, overlay 1050x672 (matching the Phase 2 SDL
  window) and a 384x240x4 opaque frame; crop off gives `[5, 0, 374, 240]`
  and overlay 1122x720; max gives `[0, 0, 384, 240]` and 1152x720; off
  again restores `frame_width` 256 and `[0, 0, 256, 240]`.
- Also checked once in headless Chromium (Playwright, a throwaway
  script driving `window.nesLoadRom`, the Wide button's `click()`,
  `nesApp.keyDown("KeyW")` and `nesApp.setCrop`) at a 1280x900
  viewport: no page errors; `canvas.width`/`height` equalled the rect
  at every step (240x224, 350x224, 374x240, 384x240, 256x240, 240x224),
  the computed `aspect-ratio` followed (`350 / 224` and so on), the box
  kept its 680 px height and widened from 729 to 1063 (16:9) and 1088
  (max) px, and the button read "Wide", "Wide 16:9", "Wide max".
  `window.nesApp.picture` exposes the rect and canvas size for such
  scripts. Not verified: how the wide picture looks with a real ROM in
  the browser (the strips are the SDL renderer's, see Phase 2) and the
  button on an actual touch device.
