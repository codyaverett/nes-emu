# NES Emulator Documentation

This directory contains all documentation, debugging notes, and test outputs for the NES emulator project.

## Directory Structure

### `/debugging/`
Contains debugging session notes and references:
- `BUGFIX_SUMMARY_2025-01-15.md` - Summary of bug fixes from January 15, 2025
- `DEBUGGING_SESSION_2025-01-15.md` - Detailed debugging session notes
- `DEBUG_REFERENCE.md` - Reference guide for debugging the emulator
- `INTERRUPT_LINE.md` - CPU NMI edge / IRQ level polling and APU frame/DMC IRQ sources (#3)
- `MAPPER_TRAIT_REFACTOR.md` - Mapper trait, one file per mapper, PPU CHR routing (#2)
- `BUS_TICK_TIMING.md` - PPU and APU advance inside each CPU bus access, DMA as real cycles (#4)
- `VBLANK_NMI_TIMING.md` - NMI line withdrawal, 2002 suppression window, sample dot, odd-frame skip (#12)
- `APU_FRAME_COUNTER.md` - frame sequencer schedule, three-cycle IRQ flag window, length counter enable/halt, power-up and reset state (#18)
- `PPU_SPRITE_PIPELINE.md` - Per-dot sprite evaluation and fetch, overflow scan bug, sprite 0 hit rules, 8-pixel background shift fix, MMC3 A12 dot timing (#11)
- `TETRIS_MMC1_FIX.md` - Tetris garbled tiles traced to an archaic DiskDude iNES header read as mapper 65; header sanitising in the cartridge loader (#22)
- `BATTERY_SAVES.md` - Battery-backed PRG RAM persisted to a .sav file next to the ROM; Mapper prg_ram accessors, System::load_battery/save_battery, hash-based dirty flush (#26)
- `CHEAT_ENGINE.md` - Game Genie and raw code decoding, ROM-read and per-frame RAM-freeze hooks in System, .cht file format, headless SMB SXIOPO proof (#31)
- `UI_FRAMEWORK.md` - In-window UI shared by the SDL binary and the web page: 8x8 bitmap font, command palette, Tool trait and pages, App state, Painter/Key/Host, --screenshot and --ui-script debug flags and the screenshot harness (#30, #52)
- `SAVE_STATES.md` - Whole-machine save states: the NESS section format field by field, Snapshot trait and Mapper save/load hooks, F5/F8 slots and the States page, the field audit method (#39)
- `WASM_WEB_BUILD.md` - SDL behind a cargo feature, the web/ wasm-bindgen wrapper crate, the 1 MB wasm stack overflow and its fix, Node smoke test and CI (#49)
- `WASM_WEB_PAGE.md` - The browser page: canvas crop, AudioWorklet ring and audio-clocked pacing, key map, headless Chromium verification, the detached-buffer pacing bug (#50)
- `WASM_WEB_DEPLOY.md` - GitHub Pages workflow on version tags, the site layout, the 500 KB wasm size budget and debug-info check (npm run check-size), local verification of the assembled site (#53)
- `WASM_WEB_STORE.md` - IndexedDB store keyed by ROM CRC-32 for battery RAM, nine state slots and cheats, the bundled cheats.json build step, HTML slot and cheat controls, export and import, headless verification (#51)
- `SHARED_OVERLAY_UI.md` - The overlay UI moved into the library behind Painter, Key and Host; SDL screenshot harness and baseline hashes, the RGBA overlay canvas on the web page, keys through key_down, rewind and slots on the web, headless verification (#52, #58-#61)

### `/plans/`
Contains implementation plans and roadmaps:
- `ACCURACY_ROADMAP.md` - Phased plan to reach test-ROM compliance (GitHub issues #1-#5)
- `TOOLS_AND_CHEATS.md` - Command palette, tool pages and cheat engine (issues #30-#33)
- `WASM_WEB.md` - Running the emulator in a web page through WebAssembly (issues #49-#53)
- `SHARED_OVERLAY_UI.md` - Painter, Key and Host abstractions that move the overlay UI into the library for both frontends (issue #52, sub-issues #58-#61)

### `/testing/`
Contains testing guides and documentation:
- `DEBUGGING_GUIDE.md` - Guide for debugging the emulator
- `TESTING_GUIDE.md` - Guide for testing the emulator
- `COMPATIBILITY_SWEEP.md` - Scripted gameplay sweep over the commercial ROMs with per-game verdicts (tests/game_sweep.rs)
- `PPU_RENDERING_DEBUG.md` - PPU rendering debugging documentation
- `test_ppu_complete.md` - Complete PPU test documentation
- `test_start.md` - Initial test documentation

#### `/testing/test_output/`
Contains test output files (screenshots and debug logs):
- `test_frame_*.ppm` - PPM image files of test frames
- `test_frame_*_debug.txt` - Debug text output for corresponding frames
- `ui/palette.png`, `ui/help.png` - Command palette and Help page captured with --screenshot (#30)
- `ui/states.png`, `ui/states-cursor.png`, `ui/states-osd.png`, `ui/states-loaded.png` - States page and the save/load OSD lines (#39)
- `ui/web-palette.png` - The same command palette drawn by the web page's overlay canvas (#52); `ui/palette.png` is the SDL one
- `web/smb-chromium.png` - The web page playing SMB in headless Chromium (#50)

### `/summaries/`
Contains summaries of fixes and improvements:
- `FIXES_SUMMARY.md` - General fixes summary
- `NES_FREEZE_FIX_SUMMARY.md` - Summary of freeze bug fixes
- `PPU_FIXES_SUMMARY.md` - Summary of PPU-related fixes
- `PPU_IMPROVEMENTS.md` - PPU improvement documentation
- `ppu_scrolling_fix.md` - PPU scrolling fix documentation

### Root Documentation
- `CHANGELOG.md` - Project changelog

## Contributing

When adding documentation:
- Place debugging session notes in `/debugging/`
- Place test outputs in `/testing/test_output/`
- Place fix summaries in `/summaries/`
- Place testing/debugging guides in `/testing/`
- Update this README if adding new categories
