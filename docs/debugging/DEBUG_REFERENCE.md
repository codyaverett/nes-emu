# Debug Features - Quick Reference

**Version**: Post-PPU Fixes (2025-01-13)

---

## 🚀 Quick Start

### Enable All Debug Features

Add to your emulator initialization:

```rust
// Enable PPU debug logging
ppu.set_debug_flags(
    true,  // Enable debug
    true,  // Log scrolling
    true,  // Log CHR access
    true   // Log sprites
);

// Enable MMC3 debug (if using Mapper 4)
if let Some(ref mut mapper4) = cartridge.borrow_mut().mapper4 {
    mapper4.set_debug(true);
}
```

### Capture Frames

```rust
// Save current frame as image
ppu.save_frame_to_ppm("frame.ppm")?;

// Save frame debug info
ppu.save_frame_debug_info("frame_debug.txt")?;
```

---

## 📋 Debug Output Reference

### PPU Scrolling Events

**Format**: `[PPU] Event: details`

#### Scroll Register Writes

```
[PPU] Write Scroll X: value=$05, fine_x=5, coarse_x=0, t=$2005
[PPU] Write Scroll Y: value=$10, fine_y=0, coarse_y=2, t=$2410
```

**What to look for**:
- `fine_x` should be 0-7
- `coarse_x` should be 0-31
- `t` is temporary VRAM address

#### VRAM Address Writes

```
[PPU] Write PPU Addr Hi: value=$20, t=$2000
[PPU] Write PPU Addr Lo: value=$00, t=$2000, v=$2000
```

**What to look for**:
- High byte written first, then low byte
- `v` gets `t` on second write

#### Scroll Increments

```
[PPU] Increment X: v=$2000 -> $2001, cycle=8
[PPU] Increment X: Wrapped from coarse_x=31, switched nametable, v=$201F -> $2400
```

**What to look for**:
- Increment X every 8 cycles
- Wraps at x=31, switches nametable ($0400 XOR)
- Should happen during rendering (cycles 1-256, 321-336)

```
[PPU] Increment Y: v=$2000 -> $2020, scanline=0, fine_y=0
[PPU] Increment Y: Wrapped at coarse_y=29, switched nametable, v=$23A0 -> $2800
```

**What to look for**:
- Increment Y at cycle 256
- Fine Y increments first (adds $1000)
- Wraps at y=29, switches nametable ($0800 XOR)

#### Register Copies

```
[PPU] Copy X: t=$2015, v=$2000 -> $2015, scanline=0, cycle=257
[PPU] Copy Y: t=$2015, v=$2000 -> $2015, scanline=261, cycle=280
```

**What to look for**:
- Copy X happens at cycle 257
- Copy Y happens during cycles 280-304 of pre-render scanline

---

### CHR Memory Access

#### A12 Rising Edge (MMC3 Scanline Counter)

```
[PPU] A12 Rising Edge detected at addr=$1000, scanline=0, cycle=5, low_cycles=8
```

**What to look for**:
- `addr` bit 12 transitions from 0 to 1
- `low_cycles` should be ≥3 (debouncing working)
- Should happen regularly during rendering

**Troubleshooting**:
- If `low_cycles` < 3: Edge filtered out (good!)
- If too many edges: Check for rapid CHR switching
- If no edges: Pattern table always in same bank

---

### Sprite Evaluation

```
[PPU] Starting sprite evaluation for scanline 1, sprite_height=8
```

**What to look for**:
- Happens at cycle 65 each scanline
- `sprite_height` is 8 or 16
- Should see once per visible scanline

---

### MMC3 Mapper Events

**Format**: `[MMC3] Event: details`

#### Bank Selection

```
[MMC3] Bank Select: value=$00, bank=0, prg_mode=0, chr_mode=0
[MMC3] Bank Data: bank=0, value=$02
[MMC3] PRG Banks Updated: mode=0, banks=[$0000, $2000, $C000, $E000]
```

**What to look for**:
- `bank` number (0-7)
- `prg_mode` (0 or 1) - affects PRG bank layout
- `chr_mode` (0 or 1) - affects CHR bank layout
- Bank addresses should be within ROM size

#### Mirroring

```
[MMC3] Mirroring: Vertical
[MMC3] Mirroring: Horizontal
```

**What to look for**:
- Changes during game (some games switch mirroring)
- Should match game's expected mirroring

#### PRG RAM Protection

```
[MMC3] PRG RAM Protect: Enabled
[MMC3] PRG RAM Protect: Disabled
```

**What to look for**:
- Usually enabled to prevent accidental writes
- Some games disable to use RAM

#### IRQ Latch & Reload

```
[MMC3] IRQ Latch: $14
[MMC3] IRQ Reload: counter reset to 0, reload flag set
```

**What to look for**:
- Latch value determines when IRQ fires
- Reload sets counter to latch value
- Common values: $14 (20), $08 (8), $10 (16)

#### IRQ Enable/Disable

```
[MMC3] IRQ Disable: IRQ acknowledged and cleared
[MMC3] IRQ Enable: IRQ acknowledged and cleared
```

**What to look for**:
- Both clear pending IRQ (important fix!)
- Games usually disable IRQ when not needed

#### IRQ Counter

```
[MMC3] IRQ Counter Dec: 20 -> 19
[MMC3] IRQ Counter Dec: 19 -> 18
...
[MMC3] IRQ Counter Dec: 1 -> 0
[MMC3] IRQ FIRED! counter=0, enabled=true
```

**What to look for**:
- Counter decrements each A12 rising edge
- Should see ~21-30 decrements per frame (one per scanline)
- IRQ fires when counter reaches 0 and enabled

**Troubleshooting**:
- If counter decrements too fast: A12 not debounced properly
- If counter never reaches 0: Latch value too high
- If IRQ doesn't fire: Check `enabled=true`

#### IRQ Reload

```
[MMC3] IRQ Counter Reload: 5 -> 20, reload_flag=true, latch=$14
```

**What to look for**:
- Happens when counter hits 0 or reload flag set
- Counter set to latch value
- Reload flag cleared

---

## 🎨 Frame Debug Info Format

### File Structure

```
Frame Debug Info - Frame #1234
Current Scanline: 42
Current Cycle: 156

PPU Registers:
  CTRL: $90
  MASK: $1E
  STATUS: $80

Scrolling State:
  v (current VRAM addr): $2105
  t (temp VRAM addr):    $2000
  x (fine X scroll):     3
  w (write latch):       false

  Coarse X: 5
  Coarse Y: 4
  Fine Y:   0
  Nametable: 0

Sprite Info:
  Sprite count: 8
  Sprite zero in secondary: true
  Sprite size: 8x8

Background Pattern Table: $0000
Sprite Pattern Table: $1000
```

### Interpreting Values

#### CTRL Register ($2000)

```
$90 = 10010000
      ││││││││
      │││││││└─ Nametable: 0 ($2000)
      ││││││└── Nametable: 0
      │││││└─── VRAM increment: +1
      ││││└──── Sprite pattern: $0000
      │││└───── BG pattern: $1000
      ││└────── Sprite size: 8x8
      │└─────── PPU master/slave: master
      └──────── NMI enabled: yes
```

#### MASK Register ($2001)

```
$1E = 00011110
      ││││││││
      │││││││└─ Grayscale: no
      ││││││└── Show BG left: yes
      │││││└─── Show sprites left: yes
      ││││└──── Show BG: yes
      │││└───── Show sprites: yes
      ││└────── Emphasize red: no
      │└─────── Emphasize green: no
      └──────── Emphasize blue: no
```

#### STATUS Register ($2002)

```
$80 = 10000000
      ││││││││
      │││││└└─ (open bus - ignore)
      │││└──── Sprite overflow: no
      ││└───── Sprite 0 hit: no
      │└────── VBlank: yes
```

#### Scroll Registers

**v (current VRAM address)**:
```
$2105 = 0010000100000101
        │││││││││││││││└ Coarse X = 5
        ││││││││││└└└└─ Coarse Y = 4
        │││││└└└└────── Nametable = 0
        └└└└────────── Fine Y = 0
```

**Calculating Position**:
- Pixel X = (coarse_x * 8) + fine_x = (5 * 8) + 3 = 43
- Pixel Y = (coarse_y * 8) + fine_y = (4 * 8) + 0 = 32

---

## 🔍 Common Debug Patterns

### Pattern 1: Detecting Sprite 0 Hit Issues

**Expected Log**:
```
[PPU] Sprite count: 8
[PPU] Sprite zero in secondary: true
(rendering happens)
Sprite 0 hit flag set
```

**If Broken**:
```
[PPU] Sprite count: 8
[PPU] Sprite zero in secondary: false  ← Problem!
(sprite 0 not in secondary OAM)
```

**Fix**: Check sprite Y coordinate - might be off-screen

### Pattern 2: Detecting IRQ Timing Issues

**Expected Log**:
```
[MMC3] IRQ Counter Dec: 20 -> 19
(20 more decrements, one per scanline)
[MMC3] IRQ Counter Dec: 1 -> 0
[MMC3] IRQ FIRED! counter=0, enabled=true
(status bar split happens here)
```

**If Broken** (firing too early):
```
[MMC3] IRQ Counter Dec: 5 -> 4
[MMC3] IRQ Counter Dec: 4 -> 3  ← Extra decrements!
[MMC3] IRQ Counter Dec: 3 -> 2  ← A12 not debounced?
[MMC3] IRQ FIRED!  ← Too early!
```

**Fix**: Check A12 debouncing - low_cycles should be ≥3

### Pattern 3: Detecting Scroll Issues

**Expected Log** (smooth scrolling):
```
[PPU] Increment X: v=$2000 -> $2001
[PPU] Increment X: v=$2001 -> $2002
[PPU] Increment X: v=$2002 -> $2003
```

**If Broken** (jumpy scrolling):
```
[PPU] Increment X: v=$2000 -> $2001
[PPU] Increment X: v=$2000 -> $2001  ← Repeated!
[PPU] Increment X: v=$2000 -> $2001  ← Not incrementing!
```

**Fix**: Check if rendering is enabled, increment happens at right cycle

---

## 📊 Debug Output Filtering

### Show Only Errors/Warnings

```bash
./nes-emu game.nes 2>&1 | grep -i "error\|warn\|fail"
```

### Show Only MMC3 Events

```bash
./nes-emu game.nes 2>&1 | grep "\[MMC3\]"
```

### Show Only IRQ Events

```bash
./nes-emu game.nes 2>&1 | grep "IRQ"
```

### Show Only Scrolling

```bash
./nes-emu game.nes 2>&1 | grep "Scroll\|Increment\|Copy"
```

### Show Only Sprite Events

```bash
./nes-emu game.nes 2>&1 | grep "Sprite"
```

### Count Events

```bash
# Count A12 edges per second
./nes-emu game.nes 2>&1 | grep "A12" | wc -l

# Count IRQ fires
./nes-emu game.nes 2>&1 | grep "IRQ FIRED" | wc -l
```

### Save to File for Analysis

```bash
./nes-emu game.nes 2>debug.log
# Then analyze:
grep "IRQ" debug.log | less
```

---

## 🎯 Quick Troubleshooting

| Symptom | Check This Log | Look For |
|---------|---------------|----------|
| Garbled graphics | `grep "CHR\|bank"` | Wrong bank numbers |
| Missing sprites | `grep "Sprite count"` | Count = 0 or wrong |
| Status bar scrolls | `grep "Sprite 0\|IRQ"` | No sprite 0 hit or IRQ not firing |
| Jumpy scrolling | `grep "Increment"` | Increments skipped or repeated |
| Screen splits wrong | `grep "MMC3.*IRQ"` | IRQ counter wrong or firing at wrong time |
| Wrong nametable | `grep "Copy\|Nametable"` | Nametable bits wrong in v register |

---

## 💡 Pro Tips

### Tip 1: Use Frame Numbers

Add frame counter to debug output:

```rust
eprintln!("[Frame {}] [PPU] Event", frame_number);
```

Makes it easy to correlate events with visual glitches

### Tip 2: Conditional Logging

Only log specific frames:

```rust
if frame_number >= 100 && frame_number <= 110 {
    ppu.set_debug_flags(true, true, true, true);
} else {
    ppu.set_debug_flags(false, false, false, false);
}
```

### Tip 3: Visual Markers

Insert visual markers in log:

```rust
eprintln!("\n========== FRAME {} START ==========\n", frame);
// ... game logic ...
eprintln!("\n========== FRAME {} END ==========\n", frame);
```

### Tip 4: Diff Tool

Compare good vs bad frames:

```bash
diff -u good_frame_debug.txt bad_frame_debug.txt
```

Shows exactly what changed

---

## 🔧 Advanced Usage

### Custom Debug Hooks

Add your own debug points:

```rust
// In PPU rendering code:
if self.scanline == 20 && self.cycle == 100 {
    eprintln!("[DEBUG] Scanline 20, Cycle 100: v=${:04X}, sprites={}",
        self.v, self.sprite_count);
}
```

### Performance Profiling

Disable debug in performance-critical sections:

```rust
// Heavy rendering loop
let old_debug = ppu.debug_enabled;
ppu.debug_enabled = false;
// ... fast rendering ...
ppu.debug_enabled = old_debug;
```

### Watchpoints

Break on specific values:

```rust
if self.v == 0x2500 {
    eprintln!("[WATCHPOINT] v reached $2500!");
    // Optionally: capture frame, dump state, etc.
}
```

---

## 📚 See Also

- `TESTING_GUIDE.md` - Systematic testing procedures
- `PPU_FIXES_SUMMARY.md` - Technical details of all fixes
- Code comments in `src/ppu/mod.rs` - Inline documentation
- Code comments in `src/cartridge/mapper4.rs` - MMC3 specifics

---

**Quick Reference Version 1.0**
**Last Updated**: 2025-01-13
