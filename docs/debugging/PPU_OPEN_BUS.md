# PPU Open Bus: Decaying I/O Bus Latch on $2000-$2007

**Date:** 2026-09-05
**Type:** Accuracy fix (PPU register interface)
**Severity:** Low (test-ROM correctness; games rarely depend on it)
**Status:** Complete for the latch; ppu_open_bus still blocked on issue 14
**Issue:** #13 (Phase 5 of docs/plans/ACCURACY_ROADMAP.md)

## Executive Summary

CPU reads of PPU registers returned 0 in every bit the hardware leaves
undriven, and reads of the write-only registers returned 0 outright. Real
hardware has an 8-bit latch on the PPU/CPU data bus (blargg's "decay
register"): every register access leaves data on it, undriven bits read
back from it, and each bit falls to 0 about 600 ms after it was last
driven high. This change adds that latch to `Ppu` with per-bit decay.

## Behaviour implemented

From `test-roms/ppu_open_bus/readme.txt`. `D` reads back from the latch
and does not refresh it; `-` is driven by the PPU and refreshes the latch.

| Register | Bits 7..0 | Notes |
|----------|-----------|-------|
| $2000, $2001, $2003, $2005, $2006 | `DDDD DDDD` | write-only; read returns the latch |
| $2002 | `---D DDDD` | VBL, sprite 0, overflow drive bits 7-5 |
| $2004 | `---- ----` | OAM byte drives all bits |
| $2007 nametable/CHR | `---- ----` | the returned (buffered) byte drives all bits |
| $2007 palette | `DD-- ----` | 6-bit palette entry drives bits 5-0 |
| any write | loads all 8 bits | including writes to $2002 |

## Implementation (src/ppu/mod.rs)

- `io_bus: u8` holds the latch; `io_bus_stamp: [u64; 8]` records the PPU
  frame (`Ppu::frame`, already incremented in `step`) in which each bit
  was last refreshed. No new call from `System` was needed.
- `refresh_io_bus(value, mask)` merges the driven bits and restamps them.
- `io_bus()` clears any bit whose stamp is older than
  `IO_BUS_DECAY_FRAMES` (36 frames, about 600 ms NTSC) and returns the
  latch. Decay is evaluated lazily on read, and skipped when the latch is
  already zero, so `step` stays untouched.
- `read_register` composes the driven bits with `io_bus()` per the table;
  `write_register` refreshes all bits before dispatching, so a write to
  the read-only $2002 still loads the latch.
- `read_oam_data` / `read_ppu_data` are unchanged. When issue 14 masks
  attribute bits 2-4 inside `read_oam_data`, the masked byte is what gets
  latched, which is what test 11 of ppu_open_bus checks.
- Palette bytes are stored as written (8 bits) in `palette`; the $2007
  path masks them to 6 bits before latching so stray high bits cannot
  leak into the open-bus bits.

## Why 36 frames

blargg's ROM reads the value back immediately after a write (test 2) and
expects it gone after a 1000 ms delay (test 3) or after 100 x 10 ms loops
that only touch registers which do not refresh the bits under test
(tests 5, 7, 9). Any threshold from a few frames up to about 55 passes;
36 matches the roughly 600 ms quoted in the readme. Hardware varies with
temperature, so games cannot rely on the exact figure.

## Verification

- Unit tests in `src/ppu/mod.rs` (`ppu::tests`): write loads the latch
  and every write-only register returns it; $2002 drives only bits 7-5
  and leaves the low bits on their old timer; $2004 refreshes all bits;
  $2007 VRAM reads latch the returned byte and palette reads keep bits
  7-6 from the latch; per-bit decay after `IO_BUS_DECAY_FRAMES`.
- `cargo test --test blargg ppu_open_bus -- --include-ignored`: tests 2-9
  pass; the ROM fails at #10 "Bits 2-4 of sprite attributes should always
  be clear when read", which is issue 14 (OAM attribute masking). The
  test stays `#[ignore]` with that reason.
- Full `cargo test` stays green: the suites that poll $2002 in loops
  (ppu_vbl_nmi 01/03, sprite_hit 11, apu_test, mmc3_test_2, oam_read)
  and nestest are unaffected because they test the flag bits with
  `bit`/`bpl`, not the whole byte.
