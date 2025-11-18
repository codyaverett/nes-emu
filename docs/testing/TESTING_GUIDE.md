# NES Emulator - PPU Testing Guide

**Version**: Post-PPU Fixes (2025-01-13)
**Purpose**: Systematic testing of PPU rendering improvements

---

## 📋 Pre-Testing Checklist

- [ ] Build release binary: `cargo build --release`
- [ ] Verify build completes successfully
- [ ] Create test output directory: `mkdir -p test_output`
- [ ] Have ROM files ready for each mapper type
- [ ] Optional: Install image viewer for PPM files

---

## 🎮 Test ROMs by Mapper

### Mapper 0 (NROM) - Simplest Mapper

**Test ROMs**:
- Donkey Kong
- Super Mario Bros
- Ice Climber
- Balloon Fight

**What to Test**:
- [ ] Background tiles render correctly
- [ ] No pattern corruption
- [ ] Sprites appear in correct positions
- [ ] No flickering or glitches
- [ ] Colors look correct

**Expected Issues**: Should work perfectly (simplest mapper)

**Debug Command**:
```bash
# Run with minimal debug output
./target/release/nes-emu path/to/smb.nes
```

---

### Mapper 1 (MMC1) - Tests CHR Banking Fix

**Test ROMs**:
- The Legend of Zelda
- Metroid
- Kid Icarus
- Mega Man
- Castlevania II

**What to Test**:
- [ ] **CHR patterns render correctly** (main fix!)
- [ ] No corrupted graphics when switching screens
- [ ] Background tiles don't glitch
- [ ] Character sprites look correct
- [ ] 4KB vs 8KB CHR banking works

**Known Issue Before Fix**: Garbled patterns due to incorrect 8KB CHR calculation

**Debug Command**:
```bash
# Enable CHR access logging
./target/release/nes-emu path/to/zelda.nes 2>&1 | grep -i "chr\|bank"
```

**Visual Test**:
1. Load Zelda
2. Check Link sprite is correct (not garbled)
3. Move between screens - tiles should be clean
4. Enter dungeon - wall patterns should be clear

---

### Mapper 2 (UxROM) - Fixed PRG Switching

**Test ROMs**:
- Mega Man
- Castlevania
- Contra
- Duck Tales

**What to Test**:
- [ ] Game boots correctly
- [ ] Level transitions work
- [ ] No crashes when switching PRG banks
- [ ] Background scrolling smooth
- [ ] Sprites render correctly

**Expected Issues**: Should work well (simple PRG banking)

---

### Mapper 3 (CNROM) - CHR Banking

**Test ROMs**:
- Q*bert
- Spy vs. Spy
- Arkanoid
- Solomon's Key

**What to Test**:
- [ ] CHR bank switches work correctly
- [ ] Pattern changes between screens
- [ ] No glitched graphics
- [ ] Sprites render properly

**Debug Command**:
```bash
# Watch CHR bank switches
./target/release/nes-emu path/to/qbert.nes 2>&1 | grep "CHR"
```

---

### Mapper 4 (MMC3) - Most Complex Tests

**Test ROMs**:
- Super Mario Bros. 3
- Mega Man 3, 4, 5, 6
- Kirby's Adventure
- Ninja Gaiden II

**Critical Tests** (main improvements were here):

#### Test 1: Sprite 0 Hit (SMB3 Status Bar)
**Steps**:
1. Load Super Mario Bros. 3
2. Enter a level (World 1-1)
3. **Watch the status bar at top**

**Expected**:
- [ ] Status bar should stay fixed at top
- [ ] Game world scrolls beneath it
- [ ] No jittering or jumping
- [ ] Clean split between status bar and game

**If Broken**: Status bar scrolls with game or jitters

**Debug**:
```bash
# Enable PPU scrolling debug
# (modify code to enable: ppu.set_debug_flags(true, true, false, false))
./target/release/nes-emu smb3.nes 2>&1 | grep "Sprite 0\|Copy"
```

#### Test 2: 8x16 Sprites (SMB3)
**Steps**:
1. Load Super Mario Bros. 3
2. Look at Mario sprite
3. Jump and watch animation

**Expected**:
- [ ] Mario sprite is complete (head+body together)
- [ ] No split or missing sprite parts
- [ ] Smooth animation when jumping
- [ ] Enemies (Goombas) render correctly

**If Broken**: Mario's head/body separated or glitched

**Debug**:
```bash
# Enable sprite debug
# (modify code: ppu.set_debug_flags(true, false, false, true))
./target/release/nes-emu smb3.nes 2>&1 | grep "Sprite"
```

#### Test 3: MMC3 IRQ Timing (Status Bar Splits)
**Steps**:
1. Load Mega Man 3
2. Enter a stage
3. Watch health/energy bars at top

**Expected**:
- [ ] Health bars stay at top
- [ ] No flickering or jumping
- [ ] Clean screen split
- [ ] No scanline artifacts

**If Broken**: Bars scroll, flicker, or have horizontal lines

**Debug**:
```bash
# Enable MMC3 IRQ debug
# (modify mapper4: mapper4.set_debug(true))
./target/release/nes-emu megaman3.nes 2>&1 | grep "MMC3\|IRQ"
```

#### Test 4: A12 Debouncing (Mega Man Series)
**Steps**:
1. Load any Mega Man game (3-6)
2. Play through a stage
3. Watch for glitches during:
   - Screen scrolling
   - Boss fights
   - Weapon switching

**Expected**:
- [ ] No spurious screen splits
- [ ] Smooth scrolling
- [ ] No random glitches
- [ ] Stable during action

**Debug**:
```bash
# Watch A12 edges
./target/release/nes-emu megaman4.nes 2>&1 | grep "A12"
```

---

## 🔍 Scrolling Tests (All Mappers)

### Horizontal Scrolling

**Test ROMs**: SMB, Zelda, Mega Man

**Steps**:
1. Walk/run to the right
2. Watch background scroll
3. Check for smooth movement

**Expected**:
- [ ] Smooth pixel-by-pixel scrolling
- [ ] No jumping or stuttering
- [ ] Tiles wrap correctly at screen edge
- [ ] Fine X scroll works (sub-tile movement)

**If Broken**: Tiles jump 8 pixels at a time (coarse scroll only)

### Vertical Scrolling

**Test ROMs**: Kid Icarus, Metroid

**Steps**:
1. Move up/down
2. Watch vertical scrolling
3. Check screen transitions

**Expected**:
- [ ] Smooth vertical movement
- [ ] Correct wrapping at top/bottom
- [ ] No glitched tiles

### Bidirectional Scrolling

**Test ROMs**: Zelda, Metroid

**Steps**:
1. Move in all 4 directions
2. Test diagonal movement
3. Check screen boundaries

**Expected**:
- [ ] Scrolls correctly in all directions
- [ ] Nametable switches work
- [ ] No corrupted tiles at edges

**Debug**:
```bash
# Enable scroll logging
./target/release/nes-emu zelda.nes 2>&1 | grep "Increment\|Copy\|Scroll"
```

---

## 📸 Frame Capture Testing

### Capturing Problem Frames

When you see a rendering glitch:

1. **Enable Frame Capture** (modify code):
```rust
// In main game loop, when glitch appears:
if frame_number == problem_frame {
    ppu.save_frame_to_ppm("debug/problem_frame.ppm")?;
    ppu.save_frame_debug_info("debug/problem_frame.txt")?;
}
```

2. **View the PPM**:
```bash
# Convert to PNG (requires ImageMagick)
convert debug/problem_frame.ppm debug/problem_frame.png

# Or open directly
open debug/problem_frame.ppm  # macOS
xdg-open debug/problem_frame.ppm  # Linux
```

3. **Analyze Debug Info**:
```bash
cat debug/problem_frame.txt
```

Look for:
- Unexpected scroll register values
- Wrong nametable selection
- Incorrect sprite counts
- Pattern table mismatches

---

## 🐛 Common Issues & Solutions

### Issue: Background Tiles Corrupted

**Symptoms**: Garbled patterns, wrong graphics
**Likely Cause**: CHR banking issue
**Test**:
```bash
# Enable CHR debug
./target/release/nes-emu game.nes 2>&1 | grep "CHR\|bank"
```
**Check**:
- Bank numbers seem reasonable?
- Banks switch at appropriate times?
- Mapper 1: 8KB vs 4KB mode correct?

### Issue: Sprites Missing or Wrong Position

**Symptoms**: Missing sprites, wrong locations
**Likely Cause**: Sprite evaluation or OAM issue
**Test**:
```bash
# Enable sprite debug
./target/release/nes-emu game.nes 2>&1 | grep "Sprite"
```
**Check**:
- Sprite count matches expectation?
- Sprite 0 in secondary OAM when expected?
- 8x8 vs 8x16 mode correct?

### Issue: Status Bar Scrolls with Game

**Symptoms**: HUD/status bar not fixed, scrolls with game
**Likely Cause**: Sprite 0 hit or IRQ timing
**Specific to**: MMC3 games (SMB3, Mega Man)
**Test**:
```bash
# Watch sprite 0 hit events
./target/release/nes-emu smb3.nes 2>&1 | grep "Sprite 0"
```
**Expected**: Should see "Sprite 0 hit" once per frame

### Issue: Screen Splits in Wrong Place

**Symptoms**: Status bar at wrong height, multiple splits
**Likely Cause**: MMC3 IRQ counter timing
**Test**:
```bash
# Watch IRQ counter
./target/release/nes-emu megaman3.nes 2>&1 | grep "MMC3.*IRQ"
```
**Check**:
- IRQ counter decrements each scanline?
- IRQ fires at expected count?
- A12 edges detected properly?

### Issue: Scrolling Jumps or Stutters

**Symptoms**: Not smooth, jumps 8 pixels
**Likely Cause**: Increment timing or fine X issue
**Test**:
```bash
# Watch scroll increments
./target/release/nes-emu game.nes 2>&1 | grep "Increment"
```
**Check**:
- Increment X called every 8 cycles?
- Fine X changing (0-7)?
- Copy X/Y happening at correct cycles?

### Issue: Colors Wrong

**Symptoms**: Wrong palette colors
**Likely Cause**: Palette mirroring or attribute table
**Not a PPU rendering issue**: Check palette RAM

---

## 📊 Test Results Template

Use this template to track test results:

```
Game: _______________
Mapper: ______________
Date: _______________

Background Rendering:
[ ] Tiles correct
[ ] No corruption
[ ] Scrolling smooth

Sprite Rendering:
[ ] Sprites visible
[ ] Correct positions
[ ] 8x16 sprites work
[ ] Sprite 0 hit works

Special Effects:
[ ] Status bar fixed
[ ] Split-screen works
[ ] No IRQ glitches

Issues Found:
1.
2.
3.

Debug Output:
(paste relevant debug output here)

Screenshots:
- Frame ___: problem_description
- Frame ___: problem_description
```

---

## 🔬 Advanced Debug Techniques

### Technique 1: Frame-by-Frame Comparison

Capture multiple frames to see progression:

```rust
// Capture every 60 frames (1 second at 60 FPS)
if frame % 60 == 0 {
    let filename = format!("debug/frame_{:04}.ppm", frame);
    ppu.save_frame_to_ppm(&filename)?;
}
```

### Technique 2: Scroll Register Tracking

Track scroll changes over time:

```bash
./target/release/nes-emu game.nes 2>&1 | grep "v=\|t=" > scroll_log.txt
```

Analyze for unexpected changes

### Technique 3: Sprite Counting

Count sprites per frame:

```bash
./target/release/nes-emu game.nes 2>&1 | grep "Sprite count" | uniq -c
```

Should be consistent per scene

### Technique 4: IRQ Timing Analysis

For MMC3 games, track IRQ timing:

```bash
./target/release/nes-emu smb3.nes 2>&1 | \
  grep "IRQ" | \
  awk '{print $NF}' | \
  sort | uniq -c
```

Look for patterns in counter values

---

## 🎯 Test Priorities

### Priority 1: Critical (Test First)
1. ✅ Mapper 0 - Basic rendering works
2. ✅ Mapper 1 - CHR banking fix verified
3. ✅ Mapper 4 - Sprite 0 hit (SMB3 status bar)
4. ✅ Mapper 4 - 8x16 sprites (SMB3 Mario)

### Priority 2: Important
5. ⬜ Mapper 4 - IRQ timing (Mega Man status bars)
6. ⬜ Mapper 2/3 - Basic CHR switching
7. ⬜ All mappers - Scrolling smoothness

### Priority 3: Nice to Have
8. ⬜ Edge cases - Sprite overflow
9. ⬜ Edge cases - x=255 sprite 0
10. ⬜ PAL timing mode

---

## 📝 Reporting Issues

If you find issues, include:

1. **Game name and mapper**
2. **Exact symptoms** (screenshot if possible)
3. **Steps to reproduce**
4. **Debug output** (relevant portions)
5. **Frame capture** if visual glitch
6. **Expected vs actual behavior**

Example:
```
Game: Super Mario Bros. 3 (Mapper 4)
Issue: Status bar scrolls with game

Steps:
1. Load ROM
2. Start game
3. Enter World 1-1
4. Walk right - status bar scrolls

Debug Output:
[PPU] Sprite 0 hit not detected
[MMC3] IRQ Counter: 20 -> 19
[MMC3] IRQ Counter: 19 -> 18
...

Expected: Status bar should stay fixed
Actual: Status bar scrolls with Mario

Frame capture: debug/smb3_broken_split.ppm
```

---

## ✅ Success Criteria

Consider testing successful when:

- [ ] Mapper 0 games run without glitches
- [ ] Mapper 1 games show correct CHR patterns
- [ ] Mapper 4 status bars are fixed in place
- [ ] Mapper 4 8x16 sprites render correctly
- [ ] Scrolling is smooth in all tested games
- [ ] No new regressions introduced

---

**Happy Testing! 🎮**

For questions or issues, refer to:
- `PPU_FIXES_SUMMARY.md` - Technical details
- `DEBUG_REFERENCE.md` - Debug feature guide
- Code comments in `src/ppu/mod.rs`
