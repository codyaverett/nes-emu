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

### `/plans/`
Contains implementation plans and roadmaps:
- `ACCURACY_ROADMAP.md` - Phased plan to reach test-ROM compliance (GitHub issues #1-#5)

### `/testing/`
Contains testing guides and documentation:
- `DEBUGGING_GUIDE.md` - Guide for debugging the emulator
- `TESTING_GUIDE.md` - Guide for testing the emulator
- `PPU_RENDERING_DEBUG.md` - PPU rendering debugging documentation
- `test_ppu_complete.md` - Complete PPU test documentation
- `test_start.md` - Initial test documentation

#### `/testing/test_output/`
Contains test output files (screenshots and debug logs):
- `test_frame_*.ppm` - PPM image files of test frames
- `test_frame_*_debug.txt` - Debug text output for corresponding frames

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
