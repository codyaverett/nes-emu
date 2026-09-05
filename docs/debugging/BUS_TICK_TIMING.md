# Bus-Tick Timing: PPU and APU Advance Inside Each Instruction

**Date:** 2026-09-05
**Type:** Architectural Change
**Issue:** #4 (Phase 4 of docs/plans/ACCURACY_ROADMAP.md)
**Status:** Complete

## Executive Summary

Until this change the CPU executed a whole instruction, returned its cycle
count, and only then did the frame loop run the PPU for three times that many
dots and the APU for that many cycles. Every `$2002` read and every
`$2005`/`$2006`/`$2007` write therefore saw PPU state that was up to seven
cycles stale, and OAM DMA was a chunked stall rather than 512 real bus
transfers. The PPU and APU now advance from inside every bus access, so
registers are observed at the cycle the access really happens.

## What changed

### `System::tick` (src/system.rs)

One CPU cycle: three PPU dots (through `ppu_and_mapper` so A12 edges and
CHR reads still reach the mapper), one APU step, the cycle counters, and
audio sampling. `read_byte` and `write_byte` call `tick` first and then
perform the access ("tick, then access"); `ppu_vbl_nmi` 01 and 03 confirm
that ordering.

### Padding to the documented cycle count

Most opcode arms still perform fewer bus accesses than the 6502 does
(indexed reads skip the dummy read, read-modify-write skips the double
write). Rather than audit 256 arms at once, `cpu_step` counts the accesses
an instruction made (`instr_cycles`) and ticks the difference to the
opcode's declared count at the end. A debug assertion fires if an
instruction ever makes more accesses than it declares, which would be a
real bug. Individual opcodes get converted to exact bus sequences when a
test demands it.

### OAM DMA as real bus cycles

The `$4014` write performs one dummy tick, one more if it started on an odd
CPU cycle, then 256 `read_byte`/`write_byte(0x2004)` pairs. Each access
ticks, so the PPU and APU keep running under the DMA. The old
`oam_dma_cycles` stall and the 4-cycle chunking in `cpu_step` are gone.

### Frame loop driven by the PPU

`run_frame_with_audio` is now `while ppu.frame == start { cpu_step() }`.
The fixed 29780-cycle budget is gone. A frame ends at the end of whichever
instruction crosses the boundary.

### Audio

`tick` accumulates samples into `System::audio_out` while `audio_capture`
is set; the frame loop drains that vector into the caller's mutex-protected
buffer once per frame. The per-cycle path never takes a lock.

### Reset takes 7 cycles

`System::reset` ticks the hardware's 7 reset cycles, so a freshly reset
machine reports CYC:7 and PPU dot 21, matching line 1 of nestest.log
without the harness having to fake either number.

### APU `$4017` reset delay

With the APU stepped at the exact write cycle, the frame-sequencer reset
must land 3 CPU cycles after a write on an even cycle and 4 after an odd
one (`Apu::frame_reset_delay`). The old post-instruction stepping had
been approximating this delay by accident; `apu_test` 4-jitter caught the
regression.

### `step_instruction`

Now simply `cpu_step()`: the PPU, APU, DMA and interrupt polling all happen
inside it.

## Verification

- nestest: all 8991 lines match on registers, CYC, and now also on the
  `PPU:` scanline/dot column (`nestest_ppu_position_matches_log`, added as
  the acceptance test for this change).
- Newly passing: `ppu_vbl_nmi` 01 and 03, `sprite_hit_tests` 11.
- No regressions across the 34 passing blargg suites or the unit tests.
- Commercial ROM fingerprints (`tests/game_frames.rs`, run with
  `--ignored --nocapture` before and after): mario, Zelda, River City
  Ransom, Final Fantasy, TMNT, Tetris and SMB2 reach the same late-frame
  images; Contra and SMB3 differ at every checkpoint after frame 30 and
  need a human look. Early-frame differences are expected because NMI now
  lands at a different instruction boundary.

## Still open after this change

- `cpu_interrupts_v2` needs interrupts sampled on the penultimate cycle of
  each instruction (and branch-specific behaviour), not at the boundary.
  Landed as issue 10; see docs/debugging/INTERRUPT_LINE.md, "Sampling
  Point". Tests 1, 2 and 4 pass; 3 and 5 wait on PPU vblank alignment and
  APU frame-flag timing respectively.
- `ppu_vbl_nmi` 02 and 05-10 need exact vblank set/clear dots and NMI
  suppression; `apu_test` 1, 5 and 6 need exact length-counter and IRQ
  flag timing. Both are Phase 5 material now that the bus is cycle-exact.
- The DMC memory reader still bypasses the bus and does not stall the CPU.
