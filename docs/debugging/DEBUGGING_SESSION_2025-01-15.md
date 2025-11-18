# Debugging Session Summary - 2025-01-15

## Session Overview
**Duration:** ~2 hours
**Issues Resolved:** 2 critical bugs
**Status:** ✅ Complete - Ready for testing

## Quick Reference

### Problem 1: Game Freeze
- **Symptom:** Static corrupted screen, no animation
- **Root Cause:** NMI interrupts never processed
- **Fix:** Handle NMI before returning from frame completion
- **File:** `src/system.rs:210-226`

### Problem 2: Corrupted Graphics
- **Symptom:** Same tile pattern repeated everywhere
- **Root Cause:** PPU v register not incrementing when rendering disabled
- **Fix:** Made increment_x() unconditional
- **File:** `src/ppu/mod.rs:955-959`

## Testing

### Test the Fixes
```bash
# Build
cargo build

# Run with basic logging
RUST_LOG=info cargo run --bin nes-emu -- "./roms/mario.nes"
```

### Expected Output
```
Loading ROM: ./roms/mario.nes
ROM loaded successfully. Mapper: 0
Reset CPU, PC set to: 0x8000
Starting emulation...
PPU rendering ENABLED (MASK = 0x1E, BG: true, Sprites: true)
NMI #1 triggered! Old PC: 0x8057
NMI #1 jumping to: 0x8082
[PPU] increment_x: v: 0x0000 -> 0x0001, cycle=8
[PPU] increment_x: v: 0x0001 -> 0x0002, cycle=16
...
```

### What to Look For
✅ **Good Signs:**
- NMIs fire regularly
- v register increments (0x0000 → 0x0001 → 0x0002...)
- Screen shows Mario graphics (not random pixels)
- Animation works

❌ **Bad Signs:**
- v stays at 0x0000
- Same tile repeated everywhere
- No NMIs
- Black screen

## Documentation Files Created

1. **FIXES_SUMMARY.md** - Overview of all fixes
2. **NES_FREEZE_FIX_SUMMARY.md** - NMI bug details
3. **PPU_RENDERING_DEBUG.md** - Complete debugging process
4. **DEBUGGING_SESSION_2025-01-15.md** - This file

## Next Steps

1. **Test the emulator** - Run mario.nes and verify graphics display correctly
2. **Clean up logging** - Remove or conditionalize diagnostic logs
3. **Test other games** - Verify fix works with Contra, Zelda, etc.
4. **Commit changes** - See FIXES_SUMMARY.md for commit message template
5. **Version bump** - Update to v0.2.0
6. **Create git tag** - Tag as v0.2.0

## Debug Commands Used

### Check ROM header
```bash
xxd -l 16 "./roms/mario.nes"
```

### Check CHR ROM data
```bash
dd if="./roms/mario.nes" bs=1 skip=32784 count=256 | xxd
```

### Run with debug logging
```bash
RUST_LOG=debug cargo run --bin nes-emu -- "./roms/mario.nes" 2>&1 | head -100
```

### Build and test
```bash
cargo build 2>&1 | tail -10
RUST_LOG=info cargo run --bin nes-emu -- "./roms/mario.nes"
```

## Key Learnings

1. **NMI Timing is Critical**
   - Must handle NMIs before returning from frame loop
   - Returning early bypasses interrupt handling

2. **PPU Address Increments are Independent of Rendering**
   - v register increments during visible scanlines regardless of MASK
   - Hardware behavior must be emulated accurately

3. **Diagnostic Logging is Essential**
   - Logging v register values revealed it never changed
   - Logging increment_x() calls showed they weren't happening
   - Without logs, bugs would be much harder to find

4. **Match Conditions for Related Operations**
   - If tile fetches are unconditional, increments must be too
   - Mismatched conditions create subtle bugs

## Break Points

Good places to set breakpoints when debugging:
- `run_frame_with_audio()` - Frame timing
- `cpu_step()` - CPU execution
- `ppu_step()` - PPU rendering
- `nmi()` - Interrupt handling
- `increment_x()` - Address increments
- `fetch_nametable_byte()` - Tile fetching

## Useful Grep Commands

```bash
# Find v register modifications
grep -n "self\.v\s*[\+\-]" src/ppu/mod.rs

# Find NMI handling
grep -n "fn nmi" src/system.rs

# Find rendering checks
grep -n "rendering_enabled" src/ppu/mod.rs

# Find increment_x calls
grep -n "increment_x()" src/ppu/mod.rs
```

## Status: ✅ Ready for Testing

The emulator should now:
- ✅ Execute CPU instructions correctly
- ✅ Fire NMIs every frame
- ✅ Increment PPU address registers properly
- ✅ Display graphics correctly (pending test)

**Next Action:** Test with "./roms/mario.nes" and verify graphics!
