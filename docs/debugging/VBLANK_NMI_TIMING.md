# Vblank Flag, NMI Timing, Suppression and the Odd-Frame Skip

**Date:** 2026-09-05
**Type:** Accuracy Fix
**Issue:** #12
**Status:** Complete

## Executive Summary

All eleven `ppu_vbl_nmi` tests pass, and `cpu_interrupts_v2` 3 with them.
Three things were missing: the NMI output was a one-shot latch that nothing
could withdraw, so a `$2002` read or NMI-disable write in the same CPU cycle
could not suppress an NMI; the CPU sampled its NMI input one dot too early
relative to the PPU; and the odd-frame dot skip did not exist.

## Changes Made

### 1. NMI as a level line with edge withdrawal (src/ppu/mod.rs)

`Ppu::update_nmi_line` recomputes `VBLANK_STARTED && NMI_ENABLE` at every
point the line can change: the flag set at (241,1), the flag clear at
(261,1), a `$2002` read, and a `$2000` write. A rising edge sets
`nmi_interrupt` for the CPU; a falling edge clears it if the CPU has not
sampled it yet. Enabling NMI while the flag is set is therefore an edge
(test 04), and disabling it, or reading `$2002`, in the cycle the flag was
set withdraws the edge (tests 06, 07, 08).

### 2. `$2002` read suppression window

Reading `$2002` on the dot before the flag would be set (scanline 241, dot
0) sets `suppress_vbl`: the flag is not set that frame and no NMI occurs
(row 04 of tests 02 and 06). Reads on dot 1 or 2 return the flag set and
clear it; the edge withdrawal above then prevents the NMI (rows 05-06).

### 3. NMI sample point (src/system.rs)

The CPU samples NMI late in each cycle, which on the PPU side falls one dot
into the following CPU cycle. `System::tick` now runs one PPU dot, samples
the NMI latch on behalf of the previous cycle (`instr_cycles - 1`), then
runs the other two dots. There is no sample immediately after a bus access
any more; the access has one dot in which to withdraw the edge. Tests 05,
06, 07 and 08 measure this to the dot and all four were off by exactly one
before this change. At the start of an instruction the previous cycle is
tick 0, which the issue 10 snapshot already treats as "pending when the
instruction began".

### 4. Odd-frame skip

On odd frames with rendering enabled the pre-render line is one dot short.
The decision is taken at dot 338 using the current mask, and dot 339 is
skipped (test 10 fails "skipped too late relative to enabling BG" with the
decision one dot later). Frame parity is the PPU's own frame counter.

## Verification

- `ppu_vbl_nmi` 01-10 and the combined ROM pass; `cpu_interrupts_v2` 3 passes.
- nestest still matches all 8991 lines on registers, CYC and PPU position.
- No change in any commercial ROM fingerprint (`tests/game_frames.rs`).
- 49 of 76 blargg suites pass (was 40).

## Still open

- `cpu_interrupts_v2` 5 and the combined ROM need exact APU frame-flag
  timing (same defect as `apu_test` 6).
- Per-cycle sprite evaluation (issue 11) is the last Phase 5 item.
