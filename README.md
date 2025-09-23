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
cargo run --release <path_to_rom.nes>
```

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
- **R**: Reset emulator
- **Escape**: Exit

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