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
