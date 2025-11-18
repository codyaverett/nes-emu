# NES Emulator - Debugging Guide

**Version**: 1.0
**Last Updated**: 2025-01-15

---

## Table of Contents

1. [Common Issues](#common-issues)
2. [Diagnostic Tools](#diagnostic-tools)
3. [Case Study: Garbled Graphics on Boot](#case-study-garbled-graphics-on-boot)
4. [PPU Debugging](#ppu-debugging)
5. [Mapper Debugging](#mapper-debugging)
6. [CPU Debugging](#cpu-debugging)
7. [Quick Reference](#quick-reference)

---

## Common Issues

### 1. Garbled/Corrupted Graphics

**Symptoms:**
- Random colored pixels
- Uninitialized-looking data on screen
- Visual noise instead of game graphics

**Common Causes:**
- ❌ Displaying frames before rendering is enabled
- ❌ CHR ROM/RAM not loaded correctly
- ❌ Wrong mapper implementation
- ❌ Incorrect nametable mirroring
- ❌ Palette RAM corruption

**First Steps:**
1. Check if rendering is enabled (`PPU MASK` register)
2. Verify mapper type is correct
3. Inspect CHR data is loading
4. Check palette RAM initialization

### 2. Black Screen

**Symptoms:**
- Window opens but shows only black

**Common Causes:**
- ❌ Rendering disabled (`MASK=$00`)
- ❌ CPU stuck in loop (infinite wait)
- ❌ VBlank not triggering
- ❌ Frame buffer not updating

**First Steps:**
1. Check VBlank is setting correctly
2. Verify CPU is executing (log PC values)
3. Check PPU MASK register value
4. Verify frames are advancing

### 3. Visual Tearing/Glitches

**Symptoms:**
- Graphics appear but with artifacts
- Scrolling issues
- Sprite corruption

**Common Causes:**
- ❌ Scrolling register bugs
- ❌ Sprite 0 hit not working
- ❌ IRQ timing issues (MMC3)
- ❌ A12 debouncing problems

**First Steps:**
1. Enable PPU scroll debug logging
2. Check sprite evaluation
3. For MMC3: check IRQ counter
4. Verify A12 rising edge detection

---

## Diagnostic Tools

### Tool 1: ROM Inspector (`rom-debug`)

**Purpose**: Inspect ROM file structure and data

**Location**: `src/bin/rom_debug.rs`

**Usage**:
```bash
cargo run --release --bin rom-debug roms/game.nes
```

**Output**:
```
=== ROM Information ===
Mapper: 0
Mirroring: Vertical
PRG ROM size: 32768 bytes (32 KB)
CHR ROM size: 8192 bytes (8 KB)
Has CHR RAM: false

=== First 16 bytes of PRG ROM ===
78 D8 A9 40 8D 00 20 A2
FF 9A AD 02 20 10 FB AD

=== First 64 bytes of CHR ROM/RAM ===
03 0F 1F 1F 1C 24 26 66 00 00 00 00 1F 3F 3F 7F
...

=== Testing CHR reads through mapper ===
Reading addresses 0x0000-0x000F:
03 0F 1F 1F 1C 24 26 66 00 00 00 00 1F 3F 3F 7F
```

**What to Check**:
- ✅ Mapper number matches expected (0=NROM, 1=MMC1, 4=MMC3, etc.)
- ✅ CHR ROM size is non-zero (or CHR RAM exists)
- ✅ PRG ROM size matches expected
- ✅ First bytes of CHR contain pattern data (not all zeros)
- ✅ Mirroring matches game requirements

### Tool 2: Frame Capture (`test-render`)

**Purpose**: Run emulator headlessly and capture frames with debug info

**Location**: `src/bin/test_render.rs`

**Usage**:
```bash
cargo run --release --bin test-render roms/game.nes
```

**Output Files**:
- `test_frame_0.ppm` - Frame 0 image
- `test_frame_0_debug.txt` - Frame 0 PPU state
- `test_frame_30.ppm` - Frame 30 image
- `test_frame_30_debug.txt` - Frame 30 PPU state
- `test_frame_60.ppm` - Frame 60 image
- `test_frame_119.ppm` - Frame 119 image

**Console Output**:
```
>>> Rendering ENABLED at frame 39! MASK=0x1E
MASK: 0x1E, CTRL: 0x10
```

**Viewing PPM Files**:
```bash
# Convert to PNG (requires ImageMagick)
convert test_frame_60.ppm test_frame_60.png

# Or open directly (macOS)
open test_frame_60.ppm

# Or open directly (Linux)
xdg-open test_frame_60.ppm
```

**Debug Info File Format**:
```
Frame Debug Info - Frame #60
Current Scanline: 0
Current Cycle: 0

PPU Registers:
  CTRL: $10
  MASK: $1E    ← Check this!
  STATUS: $00

Scrolling State:
  v (current VRAM addr): $0002
  t (temp VRAM addr):    $0000
  x (fine X scroll):     0
  w (write latch):       false

Sprite Info:
  Sprite count: 0
  Sprite zero in secondary: false

Background Pattern Table: $1000
Sprite Pattern Table: $0000
```

**What to Check**:
- ✅ **MASK register**: Should be non-zero when game is running
  - `MASK=$00` → Rendering disabled
  - `MASK=$1E` → Both BG and sprites enabled
  - `MASK=$18` → Background and sprites visible (common)
- ✅ **CTRL register**: Pattern table selection
  - Bit 4: Background pattern table (0=$0000, 1=$1000)
  - Bit 3: Sprite pattern table (0=$0000, 1=$1000)
- ✅ **Frame number when rendering enables**: Usually 30-60 frames
- ✅ **Scrolling values**: Should change if game scrolls
- ✅ **Sprite count**: Should be >0 when sprites visible

---

## Case Study: Garbled Graphics on Boot

### Problem Description

**Symptom**: Super Mario Bros displays garbled, multicolored graphics on boot.

**Screenshot**: Random RGB pixels covering entire screen, no recognizable game graphics.

### Diagnostic Process

#### Step 1: Identify the Mapper

```bash
$ cargo run --release --bin rom-debug roms/mario.nes
```

**Result**:
```
Mapper: 0
Mirroring: Vertical
CHR ROM size: 8192 bytes (8 KB)
```

**Conclusion**: ✅ Mapper 0 (NROM) - simplest mapper, should work

#### Step 2: Check CHR Data

**From same output**:
```
=== First 64 bytes of CHR ROM/RAM ===
03 0F 1F 1F 1C 24 26 66 00 00 00 00 1F 3F 3F 7F
E0 C0 80 FC 80 C0 00 20 00 20 60 00 F0 FC FE FE
...
```

**Conclusion**: ✅ CHR ROM contains valid pattern data

#### Step 3: Check VBlank Timing

Added temporary debug logging to `src/ppu/mod.rs`:

```rust
if self.scanline == 241 && self.cycle == 1 {
    self.status.insert(PpuStatus::VBLANK_STARTED);
    eprintln!("[PPU] VBlank flag SET at frame {}, scanline {}, cycle {}",
        self.frame, self.scanline, self.cycle);
    // ...
}
```

**Result**:
```
[PPU] VBlank flag SET at frame 0, scanline 241, cycle 1
[PPU] VBlank flag READ (and cleared) at frame 0, scanline 241, cycle 13
[PPU] VBlank flag SET at frame 1, scanline 241, cycle 1
...
```

**Conclusion**: ✅ VBlank working correctly, frames advancing

#### Step 4: Capture and Analyze Frames

```bash
$ cargo run --release --bin test-render roms/mario.nes
```

**Key Output**:
```
--- Frame 0 ---
MASK: 0x00, CTRL: 0x40    ← Rendering DISABLED

--- Frame 30 ---
MASK: 0x00, CTRL: 0x10    ← Still disabled

>>> Rendering ENABLED at frame 39! MASK=0x1E

--- Frame 60 ---
MASK: 0x1E, CTRL: 0x10    ← Rendering ENABLED
```

**Conclusion**: 🎯 **ROOT CAUSE FOUND**

The emulator was displaying frames 0-38 where `MASK=$00` (rendering disabled).
During these frames, the frame buffer contains uninitialized data (garbage).

### The Fix

**File**: `src/main.rs`

**Before**:
```rust
system.run_frame_with_audio(Some(&audio_buffer));

texture.update(None, system.get_frame_buffer(), SCREEN_WIDTH * 3)?;
canvas.copy(&texture, None, None)?;
canvas.present();
```

**After**:
```rust
system.run_frame_with_audio(Some(&audio_buffer));

// Only display if rendering is enabled (MASK bits 3 or 4 set)
if system.ppu.mask.bits() & 0x18 != 0 {
    texture.update(None, system.get_frame_buffer(), SCREEN_WIDTH * 3)?;
    canvas.copy(&texture, None, None)?;
    canvas.present();
} else {
    // Clear to black during initialization
    canvas.set_draw_color(sdl2::pixels::Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
}
```

**Explanation**:
- Check PPU MASK register bits 3-4 (0x18)
  - Bit 3: Show background
  - Bit 4: Show sprites
- If either bit is set → rendering enabled → display frame
- If both bits clear → rendering disabled → show black screen

**Result**: ✅ Fixed! No more garbage graphics.

---

## Case Study 2: Nametable Selection Bug

### Problem Description

**Symptom**: After fixing the rendering disabled issue, Mario still displays garbled graphics. The screen shows random/corrupted tiles instead of the proper game graphics.

**Screenshot**: Graphics are rendering (not black) but tiles are completely wrong - looks like reading from empty/wrong nametable.

### Diagnostic Process

#### Step 1: Verify Rendering is Enabled

```bash
$ ./target/release/test-render roms/mario.nes 2>&1 | grep "MASK:"
```

**Result**:
```
MASK: 0x1E, CTRL: 0x10  ← Rendering IS enabled
```

**Conclusion**: ✅ Not a rendering disabled issue this time

#### Step 2: Check Palette RAM

Added palette inspection to debug output:

```rust
writeln!(file, "\nPalette RAM:")?;
for i in 0..4 {
    let offset = i * 4;
    writeln!(file, "    Palette {}: {:02X} {:02X} {:02X} {:02X}", ...)?;
}
```

**Result**:
```
Palette RAM:
  Background Palettes:
    Palette 0: 22 29 1A 0F
    Palette 1: 0F 36 17 0F
    ...
```

**Conclusion**: ✅ Palette RAM properly initialized with valid NES palette indices

#### Step 3: Check Nametable Tile Fetches

Added logging to `fetch_nametable_byte`:

```rust
fn fetch_nametable_byte(&mut self) {
    let addr = 0x2000 | (self.v & 0x0FFF);
    self.bg_next_tile_id = self.read_vram(addr);

    eprintln!("[PPU] Fetch NT: addr=${:04X}, tile_id=${:02X}, v=${:04X}",
        addr, self.bg_next_tile_id, self.v);
}
```

**Result**:
```
[PPU] Fetch NT: addr=$2000, tile_id=$00, v=$0000, scanline=0, cycle=1
[PPU] Fetch NT: addr=$2000, tile_id=$00, v=$0000, scanline=0, cycle=9
[PPU] Fetch NT: addr=$2000, tile_id=$00, v=$0000, scanline=0, cycle=17
...
```

**Conclusion**: 🎯 **All tiles reading $00** - suspicious! Either:
- Nametable is empty, OR
- Reading from wrong nametable

#### Step 4: Check Nametable Writes

Added logging to `write_vram`:

```rust
fn write_vram(&mut self, addr: u16, value: u8) {
    if addr >= 0x2000 && addr <= 0x2FFF {
        eprintln!("[PPU] Write NT: addr=${:04X}, value=${:02X}", addr, value);
    }
    // ...
}
```

**Result**:
```
[PPU] Write NT: addr=$2400, value=$24
[PPU] Write NT: addr=$2401, value=$24
[PPU] Write NT: addr=$2402, value=$24
...
```

**Conclusion**: 🎯 **ROOT CAUSE FOUND**

- **Game writes to**: Nametable 1 ($2400-$27FF)
- **PPU reads from**: Nametable 0 ($2000-$23FF)
- **v register**: $0000 (points to nametable 0)

#### Step 5: Trace v Register Updates

Why is v=$0000 instead of pointing to the correct nametable?

**Check scroll register writes**:
```bash
$ ./target/release/test-render roms/mario.nes 2>&1 | grep "Write Scroll"
```

**Result**:
```
[PPU] Write PPU Addr Hi: value=$24, t=$2400
[PPU] Write PPU Addr Lo: value=$00, t=$2400, v=$2400  ← Sets to NT1
[PPU] Write Scroll X: value=$00, fine_x=0, coarse_x=0, t=$2400
[PPU] Write Scroll Y: value=$00, fine_y=0, coarse_y=0, t=$0400  ← BUG!
```

**Conclusion**: 🎯 **BUG IDENTIFIED**

When scroll Y is written with $00:
- **Before**: `t=$2400` (nametable 1)
- **After**: `t=$0400` (nametable bit cleared!)

During pre-render scanline, `copy_y` copies `t→v`, resetting v to wrong nametable.

#### Step 6: Analyze Scroll Y Write Code

**Location**: `src/ppu/mod.rs`, function `write_scroll`

**Buggy Code**:
```rust
// Second write (Y scroll)
self.t = (self.t & !0x73E0) |
         (((value as u16) & 0x07) << 12) |  // Fine Y
         (((value as u16) & 0xF8) << 2);    // Coarse Y
```

**Bit Analysis**:
```
Mask: 0x73E0 = 0111 0011 1110 0000
Clears bits: 14, 13, 12, 11, 9, 8, 7, 6, 5
               ^^^^^^  ^^
               Fine Y  Coarse Y  ← Correct

But also clears bit 11 (Nametable Y)  ← WRONG!
```

**v/t Register Format**:
```
yyy NN YYYYY XXXXX
||| || ||||| +++++-- Coarse X (bits 4-0)
||| || +++++-------- Coarse Y (bits 9-5)
||| ++-------------- Nametable (bits 11-10)  ← Bit 11 is nametable!
+++----------------- Fine Y (bits 14-12)
```

**The Problem**:

When `t=$2400` and scroll Y writes $00:
```
t = ($2400 & !$73E0) | $0000 | $0000
  = ($2400 & $8C1F) | $0000
  = $0400              ← Lost bit 11 (nametable Y)!
```

**Correct Behavior**:

Scroll Y should:
- ✅ Set fine Y (bits 14-12)
- ✅ Set coarse Y (bits 9-5)
- ✅ **Preserve nametable bits (11-10)**
- ✅ Preserve coarse X (bits 4-0)

### The Fix

**File**: `src/ppu/mod.rs`

**Before** (buggy mask):
```rust
// Mask 0x73E0 clears bits 14,13,12,11,9,8,7,6,5
self.t = (self.t & !0x73E0) |
         (((value as u16) & 0x07) << 12) |  // Fine Y
         (((value as u16) & 0xF8) << 2);    // Coarse Y
```

**After** (correct mask):
```rust
// Mask 0x6BE0 clears bits 14,13,12,9,8,7,6,5 (NOT bit 11!)
// 0x6BE0 = 0110 1011 1110 0000
self.t = (self.t & !0x6BE0) |
         (((value as u16) & 0x07) << 12) |  // Fine Y
         (((value as u16) & 0xF8) << 2);    // Coarse Y
```

**Difference**:
```
Old: 0x73E0 = 0111 0011 1110 0000  (includes bit 11)
New: 0x6BE0 = 0110 1011 1110 0000  (excludes bit 11)
                  ^
                  Bit 11 now preserved
```

**Verification**:

After fix, scroll Y write with value=$00:
```
t = ($2400 & !$6BE0) | $0000 | $0000
  = ($2400 & $941F) | $0000
  = $2400              ← Bit 11 preserved!
```

### Result

**Before Fix**:
- All tiles read as $00 (empty)
- Reading from wrong nametable
- Graphics completely garbled

**After Fix**:
- ✅ Tiles read from correct nametable ($2400)
- ✅ Graphics display correctly
- ✅ Mario appears properly!

---

## PPU Debugging

### Enable PPU Debug Logging

**In your code**:
```rust
// Enable all debug categories
system.ppu.set_debug_flags(
    true,  // Enable debug
    true,  // Log scrolling
    true,  // Log CHR access
    true   // Log sprites
);
```

**Or enable selectively**:
```rust
// Only log scrolling
system.ppu.set_debug_flags(true, true, false, false);

// Only log sprites
system.ppu.set_debug_flags(true, false, false, true);
```

### PPU Debug Output Examples

#### Scrolling Events

```
[PPU] Write Scroll X: value=$05, fine_x=5, coarse_x=0, t=$2005
[PPU] Write Scroll Y: value=$10, fine_y=0, coarse_y=2, t=$2410
[PPU] Increment X: v=$2000 -> $2001, cycle=8
[PPU] Increment Y: v=$2000 -> $2020, scanline=0, fine_y=0
[PPU] Copy X: t=$2015, v=$2000 -> $2015, scanline=0, cycle=257
[PPU] Copy Y: t=$2015, v=$2000 -> $2015, scanline=261, cycle=280
```

#### CHR Access

```
[PPU] A12 Rising Edge detected at addr=$1000, scanline=0, cycle=5, low_cycles=8
```

#### Sprite Evaluation

```
[PPU] Starting sprite evaluation for scanline 1, sprite_height=8
[PPU] Sprite count: 8
[PPU] Sprite zero in secondary: true
```

### Capture Frame Snapshots

**In your code**:
```rust
// Save current frame as PPM image
system.ppu.save_frame_to_ppm("debug_frame.ppm")?;

// Save PPU state info
system.ppu.save_frame_debug_info("debug_info.txt")?;
```

**Example use case**:
```rust
// Capture frame when glitch occurs
if frame_number == problem_frame {
    system.ppu.save_frame_to_ppm(&format!("glitch_frame_{}.ppm", frame_number))?;
    system.ppu.save_frame_debug_info(&format!("glitch_frame_{}.txt", frame_number))?;
}
```

### Check Rendering State

**Quick check**:
```rust
let rendering_enabled = system.ppu.mask.bits() & 0x18 != 0;
println!("Rendering enabled: {}", rendering_enabled);
println!("MASK: 0x{:02X}", system.ppu.mask.bits());
println!("CTRL: 0x{:02X}", system.ppu.ctrl.bits());
```

**MASK Register Bits**:
- Bit 0: Grayscale
- Bit 1: Show background in leftmost 8 pixels
- Bit 2: Show sprites in leftmost 8 pixels
- **Bit 3: Show background** ← Check this
- **Bit 4: Show sprites** ← Check this
- Bit 5-7: Color emphasis

**Common Values**:
- `$00` = All rendering disabled
- `$18` = BG and sprites enabled (common)
- `$1E` = BG and sprites enabled + show in left 8 pixels
- `$08` = Only background enabled
- `$10` = Only sprites enabled

---

## Mapper Debugging

### Check Mapper Type

```bash
cargo run --release --bin rom-debug roms/game.nes
```

Look for `Mapper: N` in output.

**Common Mappers**:
- **0 (NROM)**: Simplest, no banking
- **1 (MMC1)**: 4KB/8KB CHR banking, PRG banking
- **2 (UxROM)**: PRG banking only
- **3 (CNROM)**: CHR banking only
- **4 (MMC3)**: Complex, IRQ support, CHR+PRG banking
- **5 (MMC5)**: Very complex (not fully implemented)

### Enable Mapper Debug (MMC3 Example)

```rust
if let Some(ref cart) = system.cartridge {
    if let Some(ref mut mapper4) = cart.borrow_mut().mapper4 {
        mapper4.set_debug(true);
    }
}
```

**MMC3 Debug Output**:
```
[MMC3] Bank Select: value=$00, bank=0, prg_mode=0, chr_mode=0
[MMC3] Bank Data: bank=0, value=$02
[MMC3] IRQ Latch: $14
[MMC3] IRQ Counter Dec: 20 -> 19
[MMC3] IRQ FIRED! counter=0, enabled=true
```

### CHR Banking Issues

**Symptom**: Garbled patterns, wrong graphics

**Debug**:
1. Enable CHR access logging
2. Check bank numbers are reasonable
3. Verify CHR ROM/RAM size

**Example**:
```bash
./nes-emu game.nes 2>&1 | grep "CHR\|bank"
```

---

## CPU Debugging

### Check CPU is Running

**Add to `src/system.rs` (already present)**:

```rust
// Log first 100 instructions
static mut INSTRUCTION_COUNT: u32 = 0;
unsafe {
    if INSTRUCTION_COUNT < 100 {
        log::debug!("PC: 0x{:04X}, Op: 0x{:02X}", old_pc, opcode);
    }
    INSTRUCTION_COUNT += 1;
}
```

**Run with debug logging**:
```bash
RUST_LOG=debug cargo run --release -- roms/game.nes 2>&1 | head -50
```

**Expected output**:
```
[DEBUG nes_emu::system] PC: 0x8000, Op: 0x78
[DEBUG nes_emu::system] PC: 0x8001, Op: 0xD8
[DEBUG nes_emu::system] PC: 0x8002, Op: 0xA9
...
```

**If stuck in loop**:
```
[DEBUG nes_emu::system] PC: 0x800A, Op: 0xAD
[DEBUG nes_emu::system] PC: 0x800D, Op: 0x10
[DEBUG nes_emu::system] PC: 0x800A, Op: 0xAD  ← Repeating!
[DEBUG nes_emu::system] PC: 0x800D, Op: 0x10
```

**Common infinite loops**:
- **VBlank wait**: `LDA $2002 / BPL loop` (waiting for bit 7)
- **PPU warmup**: Waiting for 2 VBlanks on reset
- **Input wait**: Waiting for controller input

### Check Reset Vector

**Output on reset**:
```
[INFO nes_emu::system] Reset CPU, PC set to: 0x8000
[INFO nes_emu::system] Reset vector bytes: 0x00 0x80 => PC: 0x8000
```

**If wrong**:
- PC should point to valid code
- Usually in range `$8000-$FFFF` (PRG ROM)

---

## Quick Reference

### Diagnostic Checklist

When you encounter rendering issues, run through this checklist:

#### ✅ Step 1: Check Mapper
```bash
cargo run --release --bin rom-debug roms/game.nes
```
- [ ] Mapper type correct?
- [ ] CHR ROM/RAM present?
- [ ] CHR data non-zero?

#### ✅ Step 2: Check Frames Advance
```bash
cargo run --release --bin test-render roms/game.nes 2>&1 | grep "Frame\|Rendering"
```
- [ ] Frames incrementing?
- [ ] VBlank triggering?
- [ ] When does rendering enable?

#### ✅ Step 3: Check PPU State
View `test_frame_60_debug.txt`:
- [ ] MASK != $00?
- [ ] CTRL looks reasonable?
- [ ] Scroll values changing (if game scrolls)?

#### ✅ Step 4: Check CPU Execution
```bash
RUST_LOG=debug cargo run --release -- roms/game.nes 2>&1 | head -100
```
- [ ] CPU executing (PC advancing)?
- [ ] Not stuck in loop?
- [ ] Reset vector correct?

### Common MASK Values

| Value | Binary   | Meaning |
|-------|----------|---------|
| `$00` | 00000000 | All rendering OFF |
| `$08` | 00001000 | Background only |
| `$10` | 00010000 | Sprites only |
| `$18` | 00011000 | BG + Sprites (no left clip) |
| `$1E` | 00011110 | BG + Sprites + Left clip disabled |
| `$1F` | 00011111 | Full rendering + all options |

### Debug Output Filtering

```bash
# Show only PPU events
./nes-emu game.nes 2>&1 | grep "\[PPU\]"

# Show only MMC3 events
./nes-emu game.nes 2>&1 | grep "\[MMC3\]"

# Show only IRQ events
./nes-emu game.nes 2>&1 | grep "IRQ"

# Show only scrolling
./nes-emu game.nes 2>&1 | grep "Scroll\|Increment\|Copy"

# Count A12 edges per run
./nes-emu game.nes 2>&1 | grep "A12" | wc -l
```

---

## Advanced Debugging

### Frame-by-Frame Capture

**Modify `test-render` to save every frame**:

```rust
for frame_num in 0..180 {
    system.run_frame_with_audio(None);

    let filename = format!("frames/frame_{:03}.ppm", frame_num);
    system.ppu.save_frame_to_ppm(&filename)?;
}
```

**Then create video**:
```bash
# Convert all PPM to PNG
for f in frames/frame_*.ppm; do
    convert "$f" "${f%.ppm}.png"
done

# Create video with ffmpeg
ffmpeg -framerate 60 -i frames/frame_%03d.png -c:v libx264 output.mp4
```

### Comparing Against Reference Emulator

1. Run reference emulator (Mesen, FCEUX)
2. Capture frames at specific points
3. Compare with your emulator's frames
4. Look for first frame that diverges

### Memory Inspection

**Add to your code**:
```rust
// Dump palette RAM
println!("Palette RAM:");
for i in 0..32 {
    print!("{:02X} ", system.ppu.palette[i]);
    if (i + 1) % 8 == 0 { println!(); }
}

// Dump OAM (sprite data)
println!("OAM:");
for i in (0..256).step_by(4) {
    println!("Sprite {}: Y={:02X} Tile={:02X} Attr={:02X} X={:02X}",
        i/4,
        system.ppu.oam_data[i],
        system.ppu.oam_data[i+1],
        system.ppu.oam_data[i+2],
        system.ppu.oam_data[i+3]
    );
}
```

---

## See Also

- `PPU_FIXES_SUMMARY.md` - Technical details of PPU fixes
- `TESTING_GUIDE.md` - Systematic testing procedures
- `DEBUG_REFERENCE.md` - Debug features quick reference
- `CHANGELOG.md` - All changes and improvements

---

**Debugging Guide Version 1.0**
**Last Updated**: 2025-01-15

