# Compatibility Sweep

**Date:** 2026-09-05 (states flow, issue #41); first sweep 2026-09-05 at 0.8.0
**Emulator version:** 0.11.0 (save states, issue #39)
**Method:** `tests/sweep_states.rs` builds a save state inside gameplay for
each ROM; `tests/game_sweep.rs` loads it and plays with a per-game script,
writing frames at 400, 900, 1500 and 2400 that a human (and Claude, via
image reading) looks at.

## Why

The test ROM harness proves CPU, PPU, APU and MMC3 behaviour to the cycle,
but it only exercises what test ROMs cover. This sweep drives every ROM in
`roms/` into gameplay and looks at the frames. It is the regression check
for behaviour the harness cannot see.

The first version drove every game with one fixed script (Start four
times, hold Right, tap A) from power-on. That stalled on any game with a
menu path: SMB3 never left the map, Zelda sat on the name registration
screen, TMNT on the area 1 information screen. With save states the menu
path is scripted once per game, saved, and the sweep starts inside the
level.

## Running it

Two steps. The first only needs repeating when a recipe changes or the
state format moves (see "State files are build-bound" below).

```bash
# 1. Build roms/<stem>.sweep.state for every game (about 10 s in release)
cargo test --release --test sweep_states -- --ignored --nocapture

# 2. The sweep: loads each state, plays, writes PPM frames
SWEEP_OUT=/tmp/sweep cargo test --release --test game_sweep -- --ignored --nocapture
```

`roms/` and everything in it (ROMs, `.sav`, `.sweep.state`) is gitignored.
A missing ROM is skipped with a message. The sweep prints the mode per
game:

```text
SuperMarioBros3.nes: mapper 4
SuperMarioBros3.nes: mode state (SuperMarioBros3.sweep.state), script Hopper
SuperMarioBros.nes: mapper 66
SuperMarioBros.nes: mode menu script
```

Tuning knobs:

- `SWEEP_ONLY=<substring>` (game_sweep): only ROMs whose file name
  contains it.
- `SWEEP_TRACE=<dir>` and `SWEEP_TRACE_EVERY=<n>` (sweep_states): a PPM
  every n frames (default 50) along the recipe, which is how the recipes
  were tuned. `SWEEP_OUT=<dir>` writes the first frame after each state
  loads as `<stem>.state.ppm`.
- One recipe: `cargo test --release --test sweep_states -- --ignored zelda`.

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

Forty frames are easier to judge as a contact sheet: concatenate the
PPMs row by row into one image and write it with the same script, with
the IHDR width and height scaled to `W * cols` by `H * rows`.

## How the states are built

`tests/sweep_states.rs` has one recipe per game: a list of timed button
events (`tap` holds a button 8 frames, `hold` spans a range, `mash`
repeats a tap) and a save frame. At the save frame every button is
released, four idle frames run so no held input is baked into the image
(the `INPT` section stores the controller and a held Right would steer
the sweep before its script starts), and `System::save_state` writes
`roms/<stem>.sweep.state`.

Each state is then verified in a fresh `System`: load the ROM, load the
image, run 60 frames on it and on the recipe machine, and assert the two
frame buffers and the two `save_state` images are identical. Any recipe
whose state failed this would be reported as such; all ten pass.

| ROM | Recipe (frame: input) | Save | Where it lands |
|---|---|---|---|
| mario.nes | 150 Start; 200-400 hold Right | 404 | World 1-1, Mario past the first Goomba |
| SuperMarioBros2.nes | 150 Start; 300 Start; 450 A (Mario); 700-1000 hold Right | 1004 | World 1-1, Mario on the grass after the intro drop |
| SuperMarioBros3.nes | 150 Start; 300 Start; 600-640 hold Right; 700 A; 760 Up; 820 Right; 880 A | 1104 | World 1-1, level start with the status bar |
| zelda.nes | 150 Start; 300 Start (REGISTER); 400 A (types A); 460, 500, 540 Select (heart to REGISTER); 600 Start; 700 Start | 1104 | Overworld start screen, Link standing |
| Contra.nes | 300 Start; 600-900 hold Right | 904 | Stage 1, first sniper box |
| Teenage Mutant Ninja Turtles (USA).nes | 150 Start; 400 Start; 950 Start; 1250 Start | 1504 | Area 1 overworld, Leonardo beside the first manhole |
| final_fantasy.nes | 150, 300, 450, 600 Start; A every 40 from 720 | 2404 | Overworld outside Coneria, party named AAAA |
| river_city_ransom.nes | 150 Start; 300 Start (1P, Alex); 500 Start (speed and skill); 700-1100 hold Right | 1104 | Cross Town High, first gang members |
| Tetris.nes | 300, 450, 600, 750 Start | 904 | A-Type, level 0, first piece falling |
| 1200-in-1.nes | 150 Start (menu, Bomberman); 400 Start (Bomberman title) | 704 | Bomberman stage 1 |
| SuperMarioBros.nes | none | | Dirty header reads as mapper 66; the sweep's menu script draws garbage. Use mario.nes. |

Recipe notes, found by viewing `SWEEP_TRACE` frames:

- **SMB3.** Two Start presses reach the map (a third lands on the map and
  breaks movement). The WORLD 1 panel stays up until about frame 550, so
  Right must be held after that; then A, Up, Right, A. The first A on the
  level panel is ignored and the second enters. Mario's map X is at zero
  page $79 (START is $20, level 1 is $40).
- **Zelda.** With no save files the select screen's heart starts on
  REGISTER YOUR NAME, so the second Start opens registration directly. On
  the register screen A types the highlighted letter into file 1, and
  Select cycles the heart file 1, 2, 3, REGISTER, file 1: it skips END,
  so END is never used. Start with the heart on REGISTER returns to the
  select screen with the file named and highlighted, and Start there
  begins the game. Link is on the overworld by frame 820. Note that
  `roms/zelda.sav` exists in the main checkout; the sweep never calls
  `load_battery`, so registration always starts from an empty file list.
- **TMNT.** The copyright screen ignores Start. Start on the title, then
  the sewer intro and the area 1 information screen play by themselves.
  The information screen types two pages of text; Start only counts once
  a page has finished (about frames 800 and 1150).
- **Contra.** The title finishes scrolling in at about frame 250; Start
  before that is ignored.
- **River City Ransom.** After 1P PLAY there is a message speed and skill
  level screen that needs its own Start. Holding Right on it moves the
  cursor to ADVANCED, which is what the state has.
- **Tetris.** The legal screen ignores Start until about frame 250; then
  title, game type (A-Type) and level select each take one Start.
- **1200-in-1.** One Start launches the highlighted entry (Bomberman), one
  more on its title starts stage 1; any later Start pauses it, so no play
  script taps Start.
- **Final Fantasy.** The old menu script already reached the overworld by
  frame 2400 (the A taps name every character AAAA), so it is the recipe.

## How the sweep plays

`game_sweep.rs` keeps the four checkpoints and PPM names. In state mode
the script starts 30 frames after the load and never taps Start (it
pauses most games). The table maps ROM stem to script; an unknown stem
plays as a platformer.

| Script | Input | Games |
|---|---|---|
| Platformer | hold Right, hold A 24 frames every 40 (a full jump) | mario, SMB2, Contra, Zelda, 1200-in-1 |
| Hopper | hold Right, tap A 6 frames every 40 (a short hop) | SMB3 |
| Fighter | hold Right, tap A and B alternately every 30 | River City Ransom |
| Tetris | tap Right every 60, tap A (rotate) every 45 | Tetris |
| Walker | hold Right only (A opens the menu on an RPG overworld) | Final Fantasy |
| Sewer | hold Left 26 frames onto the manhole; from frame 230 hold Down off the ladder; from 350 hold Right and jump | TMNT |

The Hopper script exists because every longer jump tried in SMB3 1-1
(10, 14 or 24 frames, every 40, 48 or 64 frames, with or without B held
to run) put Mario on the Goomba or the piranha plant before frame 900
and the rest of the run was the map. The short hop keeps him alive at
the first pipe for all 2400 frames, which exercises the level renderer
(scrolling, sprites, MMC3 status bar split) more than the map does.

## Results at 0.11.0

All frames from state mode; SuperMarioBros.nes ran the menu script.

| ROM | Mapper | Frames 400 / 900 / 1500 / 2400 | Verdict |
|---|---|---|---|
| mario.nes | 0 | 1-1 past the first pipe; between the pipes; jumping the brick row at score 200; dies and the WORLD 1-1 card with 1 life at 2400 | OK, plays through the level |
| SuperMarioBros2.nes | 4 | 1-1 grass hill, Mario walking Right into the wall and jumping; door on the right in every frame | OK, stalls at the first wall but in play |
| SuperMarioBros3.nes | 4 | 1-1 at the first pipe in all four frames, timer counting down (293, 276, 251, 233), status bar split stable | OK, in the level for the whole run (see Hopper) |
| zelda.nes | 1 | Overworld start screen, Link walking Right; Octoroks and rocks; hearts drop 3, 3, 1.5; CONTINUE / SAVE / RETRY by 2400 | OK, plays until Link dies |
| Contra.nes | 2 | Stage 1 sniper box; bridge explosion; second sniper box with enemies; jungle further right | OK |
| Teenage Mutant Ninja Turtles (USA).nes | 1 | First sewer level, Leonardo on the ladder then fighting Foot soldiers on the lower floor; caught by 2400, LEONARDO GOT CAUGHT, WHO FIGHTS NEXT with Raphael | OK, plays into the sewer |
| final_fantasy.nes | 1 | Overworld outside Coneria, party walking Right to the coast and stopping at the water | OK |
| river_city_ransom.nes | 4 | Cross Town High; two screens further, Alex fighting Generic Dudes with a dropped weapon; the Trash Pick-Up alley | OK, scrolls through several screens |
| Tetris.nes | 1 (archaic header) | A-Type in play, pieces landing on the right side, statistics counting | OK |
| 1200-in-1.nes | 227 | Bomberman stage 1, bomb placed, balloons; killed twice; GAME OVER at 2400 | OK |
| SuperMarioBros.nes | reads as 66 | Title and garbage tiles; dirty header | Skip, use mario.nes |

No rendering defect was seen in any supported game across 2400 frames of
play from the states.

## Findings

- **State files are build-bound.** A state is refused for another ROM
  (CRC-32 in the header) and fails with a layout error if a section's
  save/load order changed since it was written, which can happen without
  a format version bump. `game_sweep.rs` prints the error, rebuilds the
  machine (a layout error is reported after earlier sections were
  applied) and falls back to the menu script, so a stale state never
  produces a half-loaded run. Rerun `sweep_states` after any change to
  `save_state`/`load_state` or a mapper's snapshot.
- **First frame after a load differs on row 0 only.** For SMB2, Zelda,
  TMNT and Final Fantasy (and River City Ransom on some runs) the
  verification prints that the first frame after the load differs from
  the recipe machine on exactly one row, row 0, and it is not flat in the
  fresh machine. The frame buffer is not in the image, and `run_frame`
  evidently returns after scanline 0 has already been partly drawn, so
  the fresh machine draws the remainder of that row over black. From the
  second frame the pictures and the complete state images are identical
  for all ten games. Cosmetic, and self-healing on the next frame, but
  it is the reason the verification compares frame 60 rather than frame
  1 (documented in `tests/sweep_states.rs`).
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
  docs/debugging/MAPPER_TRAIT_REFACTOR.md). The menu draws with no input,
  Select pages it, Down/Up move the cursor, and Start launches the
  highlighted game. Right is not a menu key on this cart, so the recipe
  simply starts the default entry.

## Results at 0.8.0 (menu script, for comparison)

| ROM | Frame 1500 / 2400 | Verdict |
|---|---|---|
| mario.nes | World 1-1, scrolling, Goomba and pipe drawn, status bar stable | OK |
| Contra.nes | Stage 1 in play, both players, mountains and water correct | OK |
| SuperMarioBros2.nes | World 1-1 in play, Mario climbing the hill | OK |
| SuperMarioBros3.nes | World 1 map with status bar (MMC3 IRQ split) | OK, script does not enter a level |
| Teenage Mutant Ninja Turtles (USA).nes | Area 1 information screen, all text and portraits correct | OK, screen needs Start to continue |
| zelda.nes | Name registration, script types letters | OK, script cannot leave the screen |
| final_fantasy.nes | Name entry at 1500, overworld by 2400 with the party sprite | OK |
| river_city_ransom.nes | Cross Town High, Alex fighting two gang members | OK |
| Tetris.nes | A-Type level select and high score table | OK |
| SuperMarioBros.nes | Not exercised; dirty header, use mario.nes | Skip |
| 1200-in-1.nes | Menu draws; first Start launches Bomberman, stage 1 board at 900 (paused by the later Start taps) | OK since issue 25 |

## What this does not cover

- Audio. Nothing headless can hear it; the APU frame counter, length
  counters and sweep units all changed in 0.7.0 and 0.8.0 and need a listen
  in the real binary.
- Deeper gameplay: later levels, bank switches that only happen on later
  stages, and save RAM. A second state per game, saved further in, would
  extend the sweep the same way.
- Timing-sensitive raster effects beyond status bars.
