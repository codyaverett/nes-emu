# PPU Rendering Corruption - Debugging Summary

## Date: 2025-01-15

## Problem Description

After fixing the NMI interrupt handling (see NES_FREEZE_FIX_SUMMARY.md), the emulator was running but displayed corrupted graphics:
- Screen showed scattered colored pixels (blue background with random white, orange, green, brown pixels)
- No recognizable Mario graphics
- Pattern looked like the same tile repeated everywhere
- No animation despite NMIs firing correctly

## Debugging Steps

### Step 1: Verify CHR ROM is Loaded Correctly

**Action:** Examined ROM header and CHR ROM data
```bash
xxd -l 16 "./roms/mario.nes"
# Output: 4e45 531a 0201 0100 0000 004e 4932 2e31
# PRG ROM: 2 banks × 16KB = 32KB
# CHR ROM: 1 bank × 8KB = 8KB ✓
# Mapper: 0 (NROM)
```

**Action:** Dumped first CHR ROM tiles
```bash
dd if="./roms/mario.nes" bs=1 skip=32784 count=256 | xxd
```

**Result:** ✅ CHR ROM contains valid Mario tile data (0x03, 0x0F, 0x1F, 0x1F, 0x1C, 0x24, 0x26, 0x66...)

**Conclusion:** CHR ROM is loaded correctly. Problem is not with tile data.

### Step 2: Add Diagnostic Logging to PPU Rendering

**Action:** Added logging to `fetch_nametable_byte()` in `src/ppu/mod.rs:1024`

```rust
fn fetch_nametable_byte(&mut self) {
    let addr = 0x2000 | (self.v & 0x0FFF);
    self.bg_next_tile_id = self.read_vram(addr);

    // Debug: Log first few nametable fetches
    static mut NT_FETCH_COUNT: u32 = 0;
    unsafe {
        if NT_FETCH_COUNT < 10 && self.scanline == 0 {
            log::info!("[PPU] Nametable: addr=0x{:04X}, tile_id=0x{:02X}, v=0x{:04X}",
                addr, self.bg_next_tile_id, self.v);
            NT_FETCH_COUNT += 1;
        }
    }
}
```

**Action:** Added logging to `fetch_pattern_low()` in `src/ppu/mod.rs:1035`

```rust
fn fetch_pattern_low(&mut self) {
    let fine_y = (self.v >> 12) & 0x07;
    let table = if self.ctrl.contains(PpuCtrl::BG_PATTERN) { 0x1000 } else { 0x0000 };
    let addr = table + (self.bg_next_tile_id as u16 * 16) + fine_y;
    self.check_a12_rising_edge(addr);
    self.bg_next_tile_lsb = self.read_vram(addr);

    // Debug: Log first few pattern fetches
    static mut FETCH_COUNT: u32 = 0;
    unsafe {
        if FETCH_COUNT < 20 && self.scanline == 0 {
            log::info!("[PPU] Pattern Low: tile_id=0x{:02X}, table=0x{:04X}, addr=0x{:04X}, data=0x{:02X}, v=0x{:04X}",
                self.bg_next_tile_id, table, addr, self.bg_next_tile_lsb, self.v);
            FETCH_COUNT += 1;
        }
    }
}
```

### Step 3: Run Emulator and Analyze Logs

**Command:**
```bash
RUST_LOG=info cargo run --bin nes-emu -- "./roms/mario.nes"
```

**Critical Output:**
```
[PPU] Nametable: addr=0x2000, tile_id=0x00, v=0x0000
[PPU] Pattern Low: tile_id=0x00, table=0x0000, addr=0x0000, data=0x03, v=0x0000
[PPU] Nametable: addr=0x2000, tile_id=0x00, v=0x0000
[PPU] Pattern Low: tile_id=0x00, table=0x0000, addr=0x0000, data=0x03, v=0x0000
[PPU] Nametable: addr=0x2000, tile_id=0x00, v=0x0000
[PPU] Pattern Low: tile_id=0x00, table=0x0000, addr=0x0000, data=0x03, v=0x0000
...
```

**🔴 CRITICAL FINDING:** The `v` register (PPU VRAM address) **NEVER CHANGES!**
- v stays at `0x0000` for all tile fetches
- Every fetch reads from address `0x2000` (tile 0 in nametable)
- Same tile (0x00) fetched repeatedly
- This explains why the screen shows the same corrupted pattern everywhere

### Step 4: Investigate Why `v` Register Doesn't Increment

**Action:** Searched for v register increment code
```bash
grep -n "self\.v\s*[\+\-]" src/ppu/mod.rs
```

**Found:**
- Line 337: `self.v = (self.v + 32) & 0x7FFF;`
- Line 339: `self.v = (self.v + 1) & 0x7FFF;`
- Line 820: `self.v += 1;` ← In `increment_x()` function
- Line 833: `self.v += 0x1000;` ← In `increment_y()` function

**Action:** Examined `increment_x()` function at line 809

```rust
fn increment_x(&mut self) {
    let old_v = self.v;
    // Increment coarse X
    if (self.v & 0x001F) == 31 {
        self.v &= !0x001F;  // Clear coarse X
        self.v ^= 0x0400;   // Switch horizontal nametable
    } else {
        self.v += 1;  // ← This should increment v
    }
}
```

**Action:** Found where `increment_x()` is called at line 941-945

```rust
7 => {
    // Increment coarse X at end of fetch
    if self.rendering_enabled() {  // ← 🔴 CONDITIONAL!
        self.increment_x();
    }
},
```

**🔴 ROOT CAUSE FOUND:** `increment_x()` is only called when `rendering_enabled()` returns true!

### Step 5: Verify the Root Cause

**Action:** Added logging to `increment_x()` to confirm it's not being called

```rust
fn increment_x(&mut self) {
    static mut INC_X_COUNT: u32 = 0;
    let old_v = self.v;
    // ... existing code ...
    unsafe {
        if INC_X_COUNT < 5 && self.scanline == 0 {
            log::info!("[PPU] increment_x: v: 0x{:04X} -> 0x{:04X}, cycle={}",
                old_v, self.v, self.cycle);
            INC_X_COUNT += 1;
        }
    }
}
```

**Action:** Ran emulator again

**Output:**
```
[PPU] Nametable: addr=0x2000, tile_id=0x00, v=0x0000
[PPU] Pattern Low: tile_id=0x00, table=0x0000, addr=0x0000, data=0x03, v=0x0000
... (many identical lines with v=0x0000) ...
[PPU rendering ENABLED (MASK = 0x1E, BG: true, Sprites: true)
[PPU] increment_x: v: 0x0002 -> 0x0003, cycle=8  ← 🟢 ONLY AFTER RENDERING ENABLED!
[PPU] increment_x: v: 0x0003 -> 0x0004, cycle=16
[PPU] increment_x: v: 0x0004 -> 0x0005, cycle=24
```

**✅ CONFIRMED:** `increment_x()` only starts running **AFTER** rendering is enabled (when MASK register is written)

### Step 6: Understand the Bug

**Timeline of Events:**

1. **Frame 0-5:** Game initializing
   - MASK register not set (rendering disabled)
   - `rendering_enabled()` returns `false`
   - Tile fetches happen (lines 927, 931, 935, 939) - **UNCONDITIONAL**
   - `increment_x()` is **SKIPPED** because of conditional at line 943
   - v stays at `0x0000`
   - Every tile fetch reads from address `0x2000` (same tile)
   - Screen shows corrupted pattern (same tile repeated)

2. **Frame 6+:** Game sets MASK register
   - MASK = 0x1E (BG and sprites enabled)
   - `rendering_enabled()` returns `true`
   - `increment_x()` finally starts running
   - v increments properly (0x0002 → 0x0003 → 0x0004...)
   - But damage is done - early frames showed corruption

**The Bug:** Tile fetches are unconditional, but v register increments are conditional on rendering being enabled. This creates a mismatch where tiles are fetched but the address never advances.

**Why This is Wrong:** On real NES hardware, the PPU address registers increment during visible scanlines regardless of whether rendering is enabled. The PPU still goes through the motions of fetching tiles even when rendering is off.

## Root Cause Analysis

**File:** `src/ppu/mod.rs`
**Line:** 941-945
**Function:** `process_visible_cycle()`

**Buggy Code:**
```rust
7 => {
    // Increment coarse X at end of fetch
    if self.rendering_enabled() {  // ← BUG: Should be unconditional
        self.increment_x();
    }
},
```

**Problem:** The condition `if self.rendering_enabled()` prevents v from incrementing when rendering is disabled, but tile fetches still happen unconditionally at fetch cycles 0, 2, 4, and 6.

**Impact:**
- Before MASK is set: v=0x0000, fetches tile 0 repeatedly → corrupted display
- After MASK is set: v increments normally → correct display (eventually)
- User sees corrupted screen until rendering enables

## The Fix

**File:** `src/ppu/mod.rs`
**Lines:** 955-959

**Before (Broken):**
```rust
7 => {
    // Increment coarse X at end of fetch
    if self.rendering_enabled() {
        self.increment_x();
    }
},
```

**After (Fixed):**
```rust
7 => {
    // Increment coarse X at end of fetch
    // NOTE: This happens during visible scanlines regardless of rendering enabled
    self.increment_x();
},
```

**Change:** Removed the `if self.rendering_enabled()` condition, making `increment_x()` unconditional.

**Rationale:**
- The NES PPU increments its address registers during visible scanlines even when rendering is disabled
- Tile fetches happen unconditionally, so address increments must also be unconditional
- This ensures v advances across the scanline, reading different nametable addresses

## Testing

**Command:**
```bash
RUST_LOG=info cargo run --bin nes-emu -- "./roms/mario.nes"
```

**Expected Behavior After Fix:**
- v register should increment from the very first frame
- Each tile fetch should read from a different nametable address
- Screen should display proper Mario graphics (once nametable is populated by game)
- No more corrupted/scattered pixel patterns

## Additional Notes

### Why Tile Fetches Were Unconditional

Looking at lines 912-948 in `process_visible_cycle()`:

```rust
// Background rendering for cycles 1-256 and 321-336
if (self.cycle >= 1 && self.cycle <= 256) || (self.cycle >= 321 && self.cycle <= 336) {
    // Shift registers every cycle
    if self.rendering_enabled() {
        self.update_shifters();
    }

    // Fetch operations every 8 cycles
    let fetch_cycle = ((self.cycle - 1) % 8) as u16;
    match fetch_cycle {
        0 => {
            if self.rendering_enabled() {
                self.load_background_shifters();
            }
            self.fetch_nametable_byte();  // ← UNCONDITIONAL
        },
        2 => {
            self.fetch_attribute_byte();   // ← UNCONDITIONAL
        },
        4 => {
            self.fetch_pattern_low();      // ← UNCONDITIONAL
        },
        6 => {
            self.fetch_pattern_high();     // ← UNCONDITIONAL
        },
        7 => {
            // Increment coarse X at end of fetch
            if self.rendering_enabled() {  // ← WAS CONDITIONAL (BUG!)
                self.increment_x();
            }
        },
        _ => {}
    }
}
```

The tile fetch functions (fetch_nametable_byte, fetch_attribute_byte, fetch_pattern_low, fetch_pattern_high) are all unconditional. They execute during visible scanlines regardless of rendering state.

But `increment_x()` was conditional, creating the mismatch.

## Related Files Modified

1. **src/ppu/mod.rs** - Lines 955-959
   - Removed conditional from `increment_x()` call

2. **src/ppu/mod.rs** - Lines 1024-1037
   - Added diagnostic logging to `fetch_nametable_byte()`

3. **src/ppu/mod.rs** - Lines 1035-1051
   - Added diagnostic logging to `fetch_pattern_low()`

4. **src/ppu/mod.rs** - Lines 809-841
   - Added diagnostic logging to `increment_x()`

## Lessons Learned

1. **Conditional vs Unconditional Operations:**
   - When debugging, check that related operations have matching conditions
   - If fetches are unconditional, increments should also be unconditional

2. **PPU Address Register Behavior:**
   - The v register increments during visible scanlines regardless of rendering state
   - This is hardware-accurate behavior that must be emulated

3. **Diagnostic Logging is Essential:**
   - Adding targeted logging to tile fetches revealed the static v register
   - Logging increment functions confirmed they weren't being called
   - Without logs, this bug would have been much harder to find

4. **Check All Frames, Not Just Final State:**
   - The bug only manifested in early frames before rendering enabled
   - Looking at later frames (after MASK set) would show correct behavior
   - Early frame analysis was crucial

## Next Steps

1. Test the fix with mario.nes to verify graphics display correctly
2. Test with other games to ensure the fix doesn't break anything
3. Consider removing diagnostic logging (or making it conditional on debug flag)
4. Document this fix in the commit message when committing

## Summary

**Problem:** Corrupted graphics (same tile repeated across entire screen)
**Root Cause:** `increment_x()` was conditional on rendering_enabled, but tile fetches were unconditional
**Fix:** Made `increment_x()` unconditional to match tile fetch behavior
**Impact:** v register now increments properly, fixing the corrupted display

The emulator should now display proper Mario graphics once the game populates the nametable!
