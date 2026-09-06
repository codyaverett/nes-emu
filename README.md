# NES Emulator

A Nintendo Entertainment System (NES) emulator written in Rust.

## Features

- 6502 CPU emulation with ~140 implemented opcodes
- PPU (Picture Processing Unit) with sprite rendering support
  - Background rendering
  - Sprite rendering with 8x8 and 8x16 modes
  - Sprite-0 hit detection
  - Sprite priority and transparency
- APU (Audio Processing Unit) basics
- Support for iNES ROM format (mapper 0)
- Controller input support
- SDL2 for video output and input handling

## Building

Install dependencies on macOS:
```bash
brew install sdl2
```

Build the project:
```bash
cargo build --release
```

## Running

```bash
cargo run --release <path_to_rom.nes> [--no-audio] [--full-frame] [--screenshot PATH:N] [--ui-script KEYS]
```

`--screenshot` and `--ui-script` are debug flags for capturing the window
headlessly; see `docs/debugging/UI_FRAMEWORK.md`.

Or after building:
```bash
./target/release/nes-emu <path_to_rom.nes>
```

### Quick Start with Super Mario Bros

```bash
# Run with normal logging
cargo run -- roms/Super_mario_brothers.nes

# Run with debug logging (shows controller inputs)
RUST_LOG=debug cargo run -- roms/Super_mario_brothers.nes
```

Or use the provided test scripts:
```bash
./test_controls.sh        # Normal mode
./test_controls_debug.sh  # Debug mode with controller logging
```

## Controls

- **Arrow Keys**: D-Pad
- **Z**: A button
- **X**: B button  
- **Enter**: Start
- **Right Shift**: Select
- Player 2: **I** / **J** / **K** / **L**: D-pad (Up / Left / Down / Right), **Apostrophe**: A, **Semicolon**: B, **Period**: Start, **Comma**: Select
- **Backquote**: Open the command palette (type to filter, Enter runs, Escape closes)
- **F1**: Help page with every key and command
- **P** / **N**: Pause or resume / frame advance
- **M**, **Plus**, **Minus**: Mute, volume up, volume down
- **R**: Reset emulator
- **F5** / **F8**: Save / load the current save-state slot (`<rom>.s1` .. `.s9`)
- **F6** / **F7**: Previous / next save-state slot; the `states` palette command lists them
- **Backspace** (hold): Rewind through the last 20 seconds, two frames back per frame; release to play on from there. Palette: `rewind N` jumps back N seconds, `rewind off` / `rewind on` stops or restarts recording
- **Escape**: Close the palette or tool page; in the game, exit

## Supported Mappers

Currently only supports mapper 0 (NROM) games, which includes many early NES titles.

## Note

This NES emulator now supports many classic NES games with mapper 0, including:
- Super Mario Bros.
- Donkey Kong
- Balloon Fight
- Ice Climber
- And other early Nintendo titles

Some limitations remain:
- Not all unofficial 6502 opcodes are implemented
- Audio output not connected to SDL (APU runs but no sound)
- Only mapper 0 (NROM) is supported
- No save states or debugging features

For best results, use mapper 0 ROM files.

## Cheats

Bundled Game Genie codes live in `cheats/`, one file per game, matched to
the ROM by CRC-32 so filenames do not matter. On first run the matching
file is copied next to the ROM as `<rom>.cht` with every cheat disabled.
Open the palette (backquote), run `cheats`, and toggle with Space; or type
`cheat add SXIOPO`. See docs/debugging/CHEAT_ENGINE.md.
