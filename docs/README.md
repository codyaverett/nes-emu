# NES Emulator Documentation

This directory contains all documentation, debugging notes, and test outputs for the NES emulator project.

## Directory Structure

### `/debugging/`
Contains debugging session notes and references:
- `BUGFIX_SUMMARY_2025-01-15.md` - Summary of bug fixes from January 15, 2025
- `DEBUGGING_SESSION_2025-01-15.md` - Detailed debugging session notes
- `DEBUG_REFERENCE.md` - Reference guide for debugging the emulator

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
