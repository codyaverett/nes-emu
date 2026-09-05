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
| 1200-in-1.nes | 227 | Solid blue at every checkpoint | FAIL: mapper 227 unsupported (issue 25) |

## Findings

- **Mapper 227 (1200-in-1 multicart)** is not implemented. The cartridge
  loader logs "Unsupported mapper 227: falling back to NROM behaviour" and
  the game never draws anything. Tracked in issue 25.
- No rendering defect was seen in any supported game across 2400 frames of
  scripted play.

## What this does not cover

- Audio. Nothing headless can hear it; the APU frame counter, length
  counters and sweep units all changed in 0.7.0 and 0.8.0 and need a listen
  in the real binary.
- Deeper gameplay: later levels, bank switches that only happen on later
  stages, and save RAM.
- Timing-sensitive raster effects beyond status bars.
