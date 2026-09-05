# Per-Dot Sprite Evaluation, Sprite Fetch Timing and Sprite 0 Hit

**Date:** 2026-09-05
**Type:** Accuracy Fix
**Issue:** #11 (last item of Phase 5, docs/plans/ACCURACY_ROADMAP.md)
**Status:** Complete

## Executive Summary

All eleven `sprite_hit_tests_2005.10.05` ROMs, all five `sprite_overflow_tests`
ROMs and `mmc3_test_2` 4-scanline_timing pass. Sprite evaluation now runs one
OAM access per dot over cycles 65-256 with the hardware's overflow-scan bug,
the eight sprite fetches are spread over 257-320 at their exact dots, and the
render units are separate from the evaluation state. Two background bugs came
out along the way: the picture was drawn 8 pixels to the right of where the
nametable puts it, and the PPU address bus was driven one dot late for the
MMC3.

## Background

`src/ppu/mod.rs` used to evaluate all 64 sprites at cycle 65 and fetch all
eight slots at cycle 257. Both wrote straight into the fields that
`render_pixel` reads for the line being drawn, so from pixel 64 onward the
current line was rendered against the *next* line's sprite count, sprite 0
flag and secondary OAM (attributes were read live from secondary OAM, which
had just been cleared to $FF, so sprites left of x = 64 got palette 3 and
behind-background priority). Sprite X ranges were computed with `u8`
wrap-around, which dropped any sprite at X >= 249. The overflow flag was set
only when sprites were enabled, not when the background alone was.

## Changes Made (src/ppu/mod.rs)

### 1. Evaluation unit, cycles 65-256

`evaluate_sprites_cycle` follows the nesdev "PPU sprite evaluation" page:

- Odd dots read `OAM[OAMADDR]` into `oam_copy_buffer`; even dots act on it.
  n and m start from OAMADDR at dot 65 (so the entry at OAMADDR is sprite 0)
  and are written back to OAMADDR after every even dot.
- An in-range Y (`scanline >= y && scanline < y + height`, compared in u16
  so Y = 239 is in range on line 239 and Y >= 240 never is) copies the four
  bytes of the entry into secondary OAM, eight dots per sprite; a miss costs
  two dots. The Y byte is written even on a miss (the next hit overwrites it).
- Once eight sprites are found, writes turn into reads of secondary OAM and
  the search continues with the hardware bug: a miss increments n *and* m
  without carry, so the "Y" compared next is byte 1 of the following sprite,
  then byte 2, byte 3 and byte 0 again (`sprite_overflow_tests` 4.Obscure).
  A hit sets the overflow flag on that even dot and consumes three more
  reads with m carrying into n; after that only n advances.
- Evaluation runs whenever rendering is enabled (background or sprites) on
  visible lines only. The pre-render line clears secondary OAM but does not
  evaluate, so line 0 never shows sprites and the eight pre-render fetches
  are all tile $FF.

### 2. Sprite fetch slots, cycles 257-320

`fetch_sprites_cycle`: slot i occupies dots 257+8i to 264+8i as two garbage
nametable fetches (the nametable and attribute addresses the background
pipeline would use), then pattern low and high. Data is read on the second
dot of each pair and the *next* access's address is put on the bus at the
end of that dot. Every slot fetches, empty ones included (secondary OAM is
$FF, which yields tile $FF through the same address path). The render units
(`sprite_patterns`, `sprite_positions`, `sprite_attributes`, `sprite_count`,
`sprite_zero_in_units`) are loaded here and nowhere else, so the line being
drawn is never disturbed. OAMADDR is held at 0 during 257-320.

### 3. `$2004` reads during rendering

Cycles 1-64 return $FF, 65-256 return the evaluation unit's copy buffer,
257-320 return the secondary OAM byte the fetch is using (Y, tile, attribute,
then X for the rest of the slot).

### 4. Sprite 0 hit

`render_pixel` already gated each layer on its enable bit and the left-8
clip bits and excluded x = 255; those rules now see the right sprite data.
The flag is set on the dot the pixel is emitted (cycle x + 1); tests 09, 10
and 11 accept that. Priority does not matter, and only unit 0 with
`sprite_zero_in_units` counts (sprite-on-sprite never hits).

### 5. Background alignment: `increment_x` at 328 and 336

The coarse-X increment was guarded with `cycle < 256`, which also skipped
the prefetch tiles at 321-336. The prefetch therefore fetched tile 0 twice
and the first fetch of the line fetched it a third time, drawing the whole
picture 8 pixels right of the nametable. Every sprite_hit ROM with a single
background tile (02, 03, 04, 08, 10) failed on its first check; the ones
with a solid screen (01, 05, 06, 07) could not see it. The guard is gone;
the increment at 256 is harmless because `copy_x` at 257 overwrites it.

### 6. PPU address bus timing for the MMC3

`mmc3_test_2` 4 measures the IRQ to the dot. Its constants say the counter
is clocked at dot 260 with sprites at $1000 (`scanline_0_08 = 6976`), 256
dots earlier with the background at $1000 (`scanline_0_10 = 6976 - 256`,
i.e. dot 4, for the first line after vblank), and 21 dots earlier than that
plus a line for later lines (`scanline_1_10 = scanline_0_10 - 21`, i.e. dot
324 of the previous line). nesdev's MMC3 page states the same 260/324.
Those are one dot before the wiki's first fetch dot (261, 5, 325): the PPU
puts the next address on the bus at the end of the previous access. Both
pipelines now do "read on the second dot, then drive the next address"
(`background_pipeline_step`, `fetch_sprites_cycle`), and the dummy
nametable fetches at 337-340 exist so A12 drops after the last prefetch.

The 9-dot low run from 336 to 4 must *not* count (otherwise line 1's clock
would land at dot 4 instead of 324), while the 68-dot sprite interval and
vblank must, so `A12_FILTER_CYCLES` went from 8 to 10. `mmc3_test_2` 1, 2,
3 and 5 still pass (their `$2006` toggles are at least 12 dots apart).

### Debugging steps, in order

1. Per-dot evaluation and fetch landed; ran the target ROMs. Overflow 1-5
   and sprite_hit 01/05/06/07/09/11 passed; 02/03/04/08 failed on their first
   check and 10 said "upper-left corner too late"; mmc3 4 reported code 3.
2. Fetched the test sources (christopherpow/nes-test-roms mirror). 02 puts a
   solid tile at $21F0 (x 128, y 120) and the sprite at (128, 119): full
   overlap, no hit. 01 passes with a full solid screen. A whole-tile
   horizontal offset was the only explanation; found the `cycle < 256` guard
   on `increment_x`. Removing it fixed 02, 03, 04, 08 and 10.
3. mmc3 4 code 3 ("scanline 0 IRQ should occur sooner when $2000=$08"): the
   sprite pattern fetch at 261 was one dot late. Moving it to 260 passed
   tests 2-7 and failed 9 (the $10 case).
4. The `-256` and `-21` constants in the source fixed the $10 clocks at dot 4
   (first line after vblank) and dot 324 (every later line). That needs the
   A12 drop at 337 and a filter that rejects the 9-dot low run across the
   line boundary: dummy fetches added, filter set to 10 dots. All 13 pass.
5. Fingerprints changed for every commercial ROM because of the 8-pixel
   shift; frames were dumped headlessly to PNG on main and on the branch
   and compared (see Verification).

## Verification

- `sprite_hit_tests_2005.10.05` 01-11, `sprite_overflow_tests` 1-5 and
  `mmc3_test_2` 1-5 pass (6 is the alternate MMC3 revision, exclusive with
  5); all un-ignored in `tests/blargg.rs`.
- No regression: `cargo test` in debug (live `debug_assert` on CPU bus
  accesses), nestest all 7 checks, ppu_vbl_nmi all 11, oam_read,
  oam_stress, ppu_open_bus.
- Unit tests in the PPU tests module: evaluation cadence and `$2004`
  visibility (`evaluation_reads_oam_on_odd_dots_and_copies_in_range_sprites`),
  background-only evaluation and no evaluation on the pre-render line, the
  overflow flag's write dot, the diagonal-scan bug, sprite 0 hit enable and
  clip rules, and the A12 rise dots 260 and 324.
- Commercial ROM fingerprints (`tests/game_frames.rs`) changed for every
  title, as they must with an 8-pixel background shift. Frame dumps
  compared by eye: SMB/Duck Hunt was a black screen on main (stuck waiting
  for a sprite 0 hit that never came) and now shows its menu; Zelda's
  select screen now draws the Link icons and heart cursor that were missing;
  SMB3, Contra and TMNT title screens are identical apart from the shift;
  Tetris' text was already garbled on main and is unchanged apart from the
  shift (unrelated, worth its own issue).

## Still open

- Sprite 0 hit is set at cycle x + 1; nesdev says the image "acts as if it
  starts at cycle 2". The blargg timing ROMs do not separate the two.
- `$2003` writes during rendering still do not corrupt OAM, and evaluation
  starting with OAMADDR & 3 != 0 follows Mesen's arithmetic but has no test
  ROM coverage.
- The A12 filter counts dots rather than M2 falling edges; 10 dots satisfies
  every ROM we have but a game that toggles `$2006` at a 9-dot spacing would
  differ.
