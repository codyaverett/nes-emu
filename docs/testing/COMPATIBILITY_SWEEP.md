# Compatibility Sweep

**Date:** 2026-09-05
**Emulator version:** 0.8.0 (test ROM suite complete, 75 of 76 blargg suites)
**Method:** `tests/game_sweep.rs`, frames viewed by a human (and by Claude
via image reading) at frames 400, 900, 1500 and 2400.

## Why

The test ROM harness proves CPU, PPU, APU and MMC3 behaviour to the cycle,
but it only exercises what test ROMs cover. This sweep drives every ROM in
`roms/` into gameplay with scripted input and looks at the frames. It is
the regression check for behaviour the harness cannot see.

## Running it

```bash
SWEEP_OUT=/tmp/sweep cargo test --release --test game_sweep -- --ignored --nocapture
```

Frames are binary PPM, 256x240. To convert to PNG with no dependencies:

```python
import struct, sys, zlib
W, H = 256, 240
raw = open(sys.argv[1], 'rb').read()
raw = raw[raw.index(b'255\n') + 4:]
rows = b''.join(b'\x00' + raw[y * W * 3:(y + 1) * W * 3] for y in range(H))
def chunk(t, d):
    return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
with open(sys.argv[2], 'wb') as f:
    f.write(b'\x89PNG\r\n\x1a\n')
    f.write(chunk(b'IHDR', struct.pack('>IIBBBBB', W, H, 8, 2, 0, 0, 0)))
    f.write(chunk(b'IDAT', zlib.compress(rows)))
    f.write(chunk(b'IEND', b''))
```

The script is deliberately simple: Start at frames 150, 300, 450 and 600,
then hold Right and tap A every 40 frames. Games that need Select, a
specific menu path, or Start to leave an info screen will stall at that
screen; that is a limit of the script, not a bug, and is noted per game.

## Results at 0.8.0

| ROM | Mapper | Frame 1500 / 2400 | Verdict |
|---|---|---|---|
| mario.nes | 0 | World 1-1, scrolling, Goomba and pipe drawn, status bar stable | OK |
| Contra.nes | 2 | Stage 1 in play, both players, mountains and water correct | OK |
| SuperMarioBros2.nes | 4 | World 1-1 in play, Mario climbing the hill | OK |
| SuperMarioBros3.nes | 4 | World 1 map with status bar (MMC3 IRQ split) | OK, script does not enter a level |
| Teenage Mutant Ninja Turtles (USA).nes | 1 | Area 1 information screen, all text and portraits correct | OK, screen needs Start to continue |
| zelda.nes | 1 | Name registration, script types letters | OK, script cannot leave the screen |
| final_fantasy.nes | 1 | Name entry at 1500, overworld by 2400 with the party sprite | OK |
| river_city_ransom.nes | 4 | Cross Town High, Alex fighting two gang members | OK |
| Tetris.nes | 1 (archaic header) | A-Type level select and high score table | OK |
| SuperMarioBros.nes | reads as 66 | Not exercised; dirty header, use mario.nes | Skip |
| 1200-in-1.nes | 227 | Menu draws; script's first Start launches Bomberman, stage 1 board with enemies at 900 (paused by the later Start taps) | OK since issue 25; menu navigates with Select (page) and Down/Up (line), not Right |

## Findings

- **SMB3 left border and right-edge seam (0.8.4).** In levels the game
  clears the PPU mask's left-column bits, so the leftmost 8 pixels are
  always blank, and it runs horizontally scrolling levels with horizontal
  mirroring (it never writes the MMC3 mirroring register in 1-1), so the
  picture wraps every 256 pixels and the rightmost columns show the
  far-left of the nametable until the game redraws them. Measured while
  running through 1-1: the stale seam is 1 to 4 pixels wide on most
  scrolling frames and 13 at worst. A self-labelling test nametable rendered
  at all 512 horizontal scroll values showed every column correct, so the
  PPU is not at fault. The binary now crops 8 pixels on all four edges by
  default.
- **Reaching an SMB3 level from a script.** Two Start presses reach the
  map (a third lands on the map and breaks movement). Then hold Right 40
  frames, wait, press A, press Up, press Right again, press A: the first A
  on the level panel is ignored and the second enters. Mario's map X is at
  zero page $79 (START is $20, level 1 is $40).

- **Final Fantasy overworld top band (0.8.3).** Walking vertically showed a
  one-frame band of wrong colours in the top eight lines. Tracing the PPU
  registers showed the emulation is exact: the vertical scroll sits at
  nametable row 29 with the correct wrap at scanline 8, and the game itself
  writes the incoming row's tiles in one vblank and its attribute bytes in
  the next, after the row is already visible. Real hardware does the same
  and a CRT hides those lines in overscan. The binary now crops 8 lines top
  and bottom by default (`--full-frame` disables it). The detector that
  found it, comparing the top 16 rows against the rest of the frame for a
  disagreeing scroll shift, is worth keeping in mind for similar reports.

- **Mapper 227 (1200-in-1 multicart)** was unimplemented at 0.8.0: the
  loader fell back to NROM and every frame was the solid backdrop. Issue 25
  added `src/cartridge/mapper227.rs` (see
  docs/debugging/MAPPER_TRAIT_REFACTOR.md). The menu now draws with no
  input, Select pages it, Down/Up move the cursor, and Start launches the
  highlighted game; Bomberman and Galaxian both reach their title or first
  stage. Right is not a menu key on this cart, so the sweep script simply
  starts the default entry.
- No rendering defect was seen in any supported game across 2400 frames of
  scripted play.

## What this does not cover

- Audio. Nothing headless can hear it; the APU frame counter, length
  counters and sweep units all changed in 0.7.0 and 0.8.0 and need a listen
  in the real binary.
- Deeper gameplay: later levels, bank switches that only happen on later
  stages, and save RAM.
- Timing-sensitive raster effects beyond status bars.
