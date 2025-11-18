# NES Emulator Freeze Fix - Debugging Summary

## Date: 2025-01-15

## Initial Problem
The NES emulator showed a corrupted static screen when running Super Mario Bros, with no animation. The game appeared completely frozen.

## Root Causes Found and Fixed

### 1. **CRITICAL: NMI Interrupt Not Being Handled**

**Location:** `src/system.rs:197-226`

**Problem:** The `run_frame_with_audio` method had a critical bug where it would return immediately when a PPU frame completed, **before** checking and handling pending NMI interrupts. This meant NMIs were set by the PPU but never processed by the CPU.

```rust
// BEFORE (BROKEN):
for _ in 0..(cpu_cycles * 3) {
    self.ppu_step();
    if self.ppu.frame != start_frame {
        return true;  // ❌ Returns before NMI check!
    }
}

// Handle NMI (THIS NEVER RAN!)
if self.ppu.nmi_interrupt {
    self.nmi();
}
```

**Fix:** Changed logic to break from PPU loop but continue to NMI handling before returning:

```rust
// AFTER (FIXED):
let mut frame_completed = false;
for _ in 0..(cpu_cycles * 3) {
    self.ppu_step();
    if self.ppu.frame != start_frame {
        frame_completed = true;
        break; // Exit loop but continue to NMI handling
    }
}

// Handle NMI (NOW RUNS!)
if self.ppu.nmi_interrupt {
    self.ppu.nmi_interrupt = false;
    self.nmi();
}

// Check if frame completed AFTER handling NMI
if frame_completed {
    self.cycles = 0;
    return true;
}
```

### 2. **Added Safety Mechanisms to Prevent Infinite Loops**

**Location:** `src/system.rs:166-195`

**Problem:** If the emulator got stuck, it would hang forever with no error messages.

**Fix:** Added multiple safety checks:
- Maximum cycle limit (3x target cycles)
- Maximum iteration counter (100,000 iterations)
- Both trigger error logs and safe exit if exceeded

### 3. **Improved CPU Error Handling**

**Location:** `src/system.rs:269-287, 2191-2214`

**Improvements:**
- Added infinite loop detection (tracks when PC doesn't change)
- Enhanced logging for HALT opcodes (KIL/JAM unofficial opcodes)
- Better logging for unknown/unimplemented opcodes
- Extended debug logging from 100 to 1000 instructions
- Added register state to debug output (A, X, Y, SP)

### 4. **Added Comprehensive Debug Logging**

**PPU MASK Register Logging** (`src/ppu/mod.rs:358-375`):
- Logs when rendering is enabled/disabled
- Shows which rendering modes are active (BG/Sprites)

**PPU Frame Counter Logging** (`src/ppu/mod.rs:715-718`):
- Logs frame completions every 60 frames
- Helps verify PPU timing is correct

**NMI Execution Logging** (`src/system.rs:2302-2320`):
- Logs first 10 NMI triggers
- Shows old PC, NMI vector address, and jump target
- Confirms NMI handling is working

## Current Status

### ✅ WORKING
- CPU executes instructions correctly
- CPU resets to proper address (0x8000)
- PPU VBLANK wait loops complete successfully
- **NMI interrupts are now being triggered and handled**
- PPU rendering gets enabled (MASK = 0x1E)
- Game main loop runs correctly at 0x8057
- NMI handler executes at 0x8082 every frame

### ❌ STILL BROKEN
- Screen shows corrupted/static image
- No animation visible
- Game doesn't progress visually

## Understanding the Execution Flow

The emulator is now working as expected from a CPU/interrupt perspective:

1. **Main Loop (0x8057):** `JMP $8057` - infinite loop
   - This is CORRECT behavior for NES games
   - The CPU sits here waiting for NMIs

2. **NMI Handler (0x8082):** Runs every frame (~60 Hz)
   - All game logic happens here
   - Updates sprites, background, sound, etc.
   - Returns via RTI back to main loop at 0x8057

3. **Expected Behavior:**
   - CPU appears "stuck" at 0x8057 between NMIs
   - NMI fires every ~16.67ms (60 FPS)
   - Game logic runs in NMI, then returns

## Next Steps for Complete Fix

The NMI fix resolved the core emulation issue. The remaining problem is **PPU rendering/display**:

### Potential Issues to Investigate:
1. **PPU Rendering Pipeline**
   - Check if background tiles are being fetched correctly
   - Verify sprite rendering is working
   - Check pattern table reads
   - Verify palette reads

2. **Frame Buffer**
   - Ensure frame buffer is being updated by PPU
   - Check if frame buffer data is valid
   - Verify buffer is being displayed to screen

3. **Scrolling/Addressing**
   - Check PPU address registers (v, t registers)
   - Verify scrolling implementation
   - Check nametable mirroring

4. **Timing Issues**
   - Verify PPU cycle timing is accurate
   - Check scanline timing
   - Ensure PPU/CPU synchronization

## ROM Compatibility Notes

- **mario.nes** (Mapper 0 - NROM): Correct ROM, emulator works
- **SuperMarioBros.nes** (Mapper 66 - GxROM): Wrong ROM or hack, uses unsupported mapper
  - Mapper 66 is not fully implemented
  - Use mario.nes instead for testing

## Files Modified

1. `src/system.rs` - Fixed NMI handling, added safety checks, improved logging
2. `src/ppu/mod.rs` - Added MASK register and frame counter logging
3. `src/main.rs` - No changes needed

## Test Command

```bash
RUST_LOG=info cargo run --bin nes-emu -- "./roms/mario.nes"
```

## Expected Log Output (Correct Behavior)

```
Loading ROM: ./roms/mario.nes
ROM loaded successfully. Mapper: 0
Reset CPU, PC set to: 0x8000
Starting emulation...
PPU rendering ENABLED (MASK = 0x1E, BG: true, Sprites: true)
NMI #1 triggered! Old PC: 0x8057
NMI #1 jumping to: 0x8082
[CPU appears stuck at PC: 0x8057 messages are NORMAL]
NMI #2 triggered! Old PC: 0x8057
NMI #2 jumping to: 0x8082
...
```

The "CPU appears stuck" messages are **expected behavior** - the CPU is supposed to loop at 0x8057 waiting for NMIs!
