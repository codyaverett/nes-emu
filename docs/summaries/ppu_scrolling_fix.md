# PPU Scrolling Artifacts Fix

## Issues Identified
The rapid offset artifacts were caused by:
1. Scrolling registers being updated even when rendering was disabled
2. Incorrect timing of scroll increment operations
3. Shift registers not being properly reset

## Fixes Applied

### 1. Conditional Rendering Pipeline
- Background operations now only run when `SHOW_BG` is enabled
- Sprite operations only run when `SHOW_SPRITES` is enabled
- This prevents spurious scroll register updates when rendering is off

### 2. Corrected Increment Timing
- X increment now happens after tile data is loaded (cycle 0 of each 8-cycle group)
- Fixed timing for prefetch cycles (328 and 336)
- Only increments when actually fetching tiles

### 3. Proper Reset State
- Added reset for all shift registers
- Ensures clean state on PPU reset
- Prevents garbage data from causing visual artifacts

### 4. Separated Background/Sprite Logic
- Background rendering only updates when background is shown
- Sprite evaluation only runs when sprites are shown
- Prevents cross-contamination of rendering state

## Technical Details

The core issue was that `increment_x()` was being called even when rendering was disabled, causing the VRAM address register (v) to change rapidly. This created the "offset artifacts" as the PPU would jump around in memory.

The fix ensures that:
- Scroll increments only happen during active rendering
- The rendering pipeline respects the PPUMASK settings
- Shift registers maintain consistent state

## Testing
The PPU should now:
- Show stable images when rendering is disabled
- Properly scroll only when games intend to scroll
- Not show rapid flickering or offset artifacts
- Handle games that toggle rendering on/off correctly