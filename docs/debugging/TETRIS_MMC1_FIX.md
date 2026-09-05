# Tetris Garbled Title Screen: Archaic iNES Header, Not MMC1

**Date:** 2026-09-05
**Type:** Bug Fix (cartridge loading)
**Issue:** #22
**Status:** Complete

## Executive Summary

`roms/Tetris.nes` rendered its copyright and title screens with the right
layout but the wrong tiles. The issue pointed at MMC1 CHR banking; the MMC1
code was fine. The dump has an archaic iNES header with the "DiskDude"
signature in bytes 7-15, so byte 7 reads `0x44` and the mapper number came
out as `0x40 | 1 = 65`. The cartridge was being built as an Irem H3001
board, which ignores every MMC1 register write, so the PPU rendered from CHR
bank 0 in a 128 KB CHR image whose text tiles live elsewhere.

`Cartridge::load_from_bytes` now applies the standard archaic-header
heuristic: when bytes 12-15 are nonzero (or byte 7 bits 2-3 read `01`) the
upper mapper nibble in byte 7 is ignored. Tetris loads as mapper 1 and both
title screens render correctly. No other ROM's fingerprint changed.

## Debugging Steps

### 1. Reproduce headlessly

A temporary `#[ignore]` integration test loaded the ROM, ran 400 frames with
Start tapped at frame 150, and wrote `System::get_frame_buffer()` (256x240
RGB) to a file. The stdlib-only `topng.py` from the appendix of
`MAPPER_TRAIT_REFACTOR.md` turned that into a PNG.

Before the fix the frame at 120 and 400 was the copyright screen with the
text rows in the right places but every glyph drawn from the wrong pattern:
a scatter of coloured block fragments and a few real letters (`S R`, `1987`,
`1989` happened to land on readable tiles). That matches the issue report
and rules out a nametable or scroll problem: the *positions* are right.

### 2. Check the header before the mapper

The issue's suspect list had the iNES header third, but it is the cheapest
to check and `SuperMarioBros.nes` had already been caught reading as mapper
66 from a dirty byte 7. Hex dump of the first 16 bytes:

```text
00000000: 4e45 531a 0810 1144 6973 6b44 7564 6521  NES....DiskDude!
```

| Byte | Value | Meaning |
|------|-------|---------|
| 4 | `0x08` | 8 x 16 KB PRG (128 KB) |
| 5 | `0x10` | 16 x 8 KB CHR ROM (128 KB) |
| 6 | `0x11` | mapper low nibble 1, vertical mirroring |
| 7 | `0x44` | ASCII `D` from the DiskDude signature |
| 8-15 | `iskDude!` | signature, not iNES data |

`(flags_7 & 0xF0) | (flags_6 >> 4)` gives 65. Adding a print of
`cart.mapper_id` to the dump test confirmed: `mapper 65 mirroring Vertical`.
The file size (16 + 262144 bytes) matches the PRG and CHR sizes exactly, so
the rest of the header is sound.

The same scan over every ROM in `roms/`:

| ROM | byte 6 | byte 7 | bytes 8-15 | mapper before | after |
|-----|--------|--------|------------|---------------|-------|
| 1200-in-1 | 30 | e0 | zeros | 227 | 227 |
| Contra | 21 | 00 | zeros | 2 | 2 |
| SuperMarioBros | 21 | 40 | zeros | 66 | 66 |
| SuperMarioBros2 | 40 | 00 | zeros | 4 | 4 |
| SuperMarioBros3 | 40 | 00 | zeros | 4 | 4 |
| TMNT (USA) | 10 | 00 | zeros | 1 | 1 |
| Tetris | 11 | 44 | `iskDude!` | 65 | 1 |
| final_fantasy | 13 | 00 | zeros | 1 | 1 |
| mario | 01 | 00 | `...NI2.1` | 0 | 0 |
| river_city_ransom | 40 | 00 | zeros | 4 | 4 |
| zelda | 12 | 00 | zeros | 1 | 1 |

Two things to note from the table. `mario.nes` also has junk in bytes 12-15
and now takes the archaic path, but its byte 7 is zero so the mapper number
is unchanged (its fingerprints are byte-identical). `SuperMarioBros.nes` has
`0x40` in byte 7 with clean padding, so no header heuristic can tell it from
a genuine GxROM dump; it stays mapper 66 and is out of scope here.
(`MAPPER_TRAIT_REFACTOR.md` describes TMNT as MMC3; the dump in `roms/` is
mapper 1.)

### 3. Why mapper 65 produces exactly this picture

`Mapper65` powers up with CHR banks 0-7 mapped to 1 KB slots 0-7, i.e. the
first 8 KB of the 128 KB CHR image, and its CHR selects live at
`$B000-$B007`, addresses Tetris never writes. Tetris programs the MMC1
through the serial port at `$8000`, `$A000`, `$C000` and `$E000`; on the
H3001 those addresses are the three 8 KB PRG bank registers (and `$E000` is
nothing), so each serial data bit rewrites a PRG bank instead of shifting
into a CHR select. The last 8 KB of PRG is fixed at `$E000` on both boards,
so the reset vector and the code that draws the copyright text still run
and write the nametable correctly; only the pattern data is wrong.

### 4. Fix

`src/cartridge/mod.rs`, `load_from_bytes`: detect an archaic header
(`data[12..16]` nonzero, or `(flags_7 & 0x0C) == 0x04`) and use only
`flags_6 >> 4` as the mapper number. A warning is logged with the raw bytes
7-15 whenever this changes the result, so the next dirty dump is a one-line
diagnosis. Byte 6 (mirroring, battery, trainer) is trusted as before.

NES 2.0 headers (byte 7 bits 2-3 = `10`) are checked first and bypass the
heuristic: bytes 12-15 are real fields there (timing, VS type, misc ROMs,
default expansion device, which is commonly `0x01`) and byte 7's mapper
nibble is valid, so without the guard a NES 2.0 dump of any mapper above 15
would have lost its upper nibble. The rest of the NES 2.0 header is still
not parsed (it never was).

### 5. Verify

After the fix the dump test printed `mapper 1 mirroring Vertical`, and the
frames were:

- **f120, f400:** the copyright screen, fully legible: "TM AND (c) 1987
  V/O ELECTRONORGTECHNICA ("ELORG") TETRIS LICENSED TO NINTENDO (c) 1989
  NINTENDO ALL RIGHTS RESERVED ORIGINAL CONCEPT, DESIGN AND PROGRAM BY
  ALEXEY PAZHITNOV", white text with the orange and blue highlights in
  the right words.
- **f700 (Start at 150 and 500):** the title screen: grey tetromino border
  all the way round, the striped TETRIS logo, St Basil's cathedral, "PUSH
  START" and "(c) 1989 Nintendo".
- **f1000 (Start again at 800):** the GAME TYPE / MUSIC TYPE menu with the
  A-TYPE selection box highlighted.

`tests/game_frames.rs` before and after: only `Tetris.nes` changed
(`093f0db6e295fa0d` to `df034e516d9844bb` at all four checkpoints; the game
sits on the copyright screen for the whole 400-frame window). Every other
ROM, including `mario.nes` with its own junk bytes, is byte-identical.

Unit tests added to `src/cartridge/mod.rs`:

- `archaic_header_signature_does_not_corrupt_mapper_number` builds a
  synthetic mapper 1 image, overwrites bytes 7-15 with the DiskDude
  signature, and asserts mapper 1, vertical mirroring, MMC1 power-on PRG
  mode 3 and a serial CHR bank load reaching `ppu_read`. On the old code
  this reported mapper 65.
- `archaic_flag_bits_alone_ignore_byte_7` covers the `0x04` flag-bit rule
  and asserts a clean `0x40` byte 7 still yields mapper 65, which is the
  guard that `SuperMarioBros.nes` and the unknown-mapper fallback behave as
  before.
- `nes2_header_keeps_upper_mapper_nibble_despite_nonzero_tail` sets the
  NES 2.0 flag bits and a nonzero byte 15 on a mapper 65 image and asserts
  the mapper number survives. Without the NES 2.0 guard this fails with 1.

## Reproducing the frame dump

Symlink `roms/` into the checkout (it is gitignored), add this ignored test
under `tests/`, run it with
`DUMP_OUT=/path/to/out/tetris.rgb cargo test --release --test tetris_dump -- --ignored --nocapture`
(it writes `tetris.rgb.120`, `.400`, `.700`, `.1000`), and convert each
with `topng.py` from the appendix of `MAPPER_TRAIT_REFACTOR.md`. Delete the
test before committing.

```rust
use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;

#[test]
#[ignore]
fn dump_tetris() {
    let cart = Cartridge::load_from_file("roms/Tetris.nes").unwrap();
    println!("mapper {} mirroring {:?}", cart.mapper_id, cart.header_mirroring);
    let mut sys = System::new();
    sys.load_cartridge(cart);
    let out = std::env::var("DUMP_OUT").unwrap();
    for f in 0..1200 {
        if f == 150 || f == 500 || f == 800 {
            sys.controller1.press(ControllerButton::START);
        } else if f == 160 || f == 510 || f == 810 {
            sys.controller1.release(ControllerButton::START);
        }
        sys.run_frame();
        for cp in [120, 400, 700, 1000] {
            if f == cp {
                std::fs::write(format!("{out}.{cp}"), sys.get_frame_buffer()).unwrap();
            }
        }
    }
}
```

## Lessons

- When a game's layout is right and only the tiles are wrong, print the
  mapper number before reading mapper code. A wrong board explains "every
  register write is ignored" far more often than a subtle bank bug does.
- Dumps from the DiskDude era are common; bytes 8-15 of any iNES header
  should be looked at, not just bytes 6 and 7.
