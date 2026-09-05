# APU Frame Counter, Length Counters and Power/Reset State

**Date:** 2026-09-05
**Type:** Accuracy Fix
**Issue:** #18
**Status:** Complete

## Executive Summary

All eight `apu_test` ROMs and the combined ROM, all six `apu_reset` ROMs,
and `cpu_interrupts_v2` 5 with its combined ROM pass. The frame sequencer
was a fixed 7457-cycle divider whose IRQ flag pulsed once; it is now the
nesdev schedule counted in CPU cycles, with the flag raised on three
consecutive cycles and a 29830-cycle period. Length counters ignored the
halt flag and reloaded while disabled. Power-up and reset did not model the
implicit `$4017` write, and reset wiped register state that hardware keeps.

## Background

blargg's tests measure everything relative to a `$4017` write made on a
known cycle parity (`sync_apu` calibrates the parity by reading the flag
exactly 29831 cycles after a write). The readmes only list failure codes,
so the sources of the tests (christopherpow/nes-test-roms, not vendored
here) were used to work out the cycle each read happens on. What they pin
down, with the write on cycle W and the sequencer restart landing on
R = W + 3:

| Event (4-step) | CPU cycle after R | Test |
|----------------|-------------------|------|
| quarter (envelope, linear) | 7457 | nesdev 3728.5 APU |
| quarter + half (length, sweep) | 14913 | apu_test 5 #2/#3: length changes between reads at W+14915 and W+14916 |
| quarter | 22371 | nesdev 11185.5 APU |
| IRQ flag | 29828 | apu_test 6 #2/#3: clear at W+29830, set at W+29831 |
| quarter + half + IRQ flag | 29829 | apu_test 5 #4/#5: second length change at W+29832 |
| IRQ flag, wrap to 0 | 29830 | apu_test 6 #4/#5: a read at W+29832 clears it, a read at W+29836 sees it again; after a clearing read at W+29833 it stays clear |

5-step mode: quarter at 7457, 14913, 22371 and 37281; half at 14913 and
37281; wrap at 37282; the quarter and half units also clock at R itself.
apu_test 5 #8-#13 measure length changes at W+14916, W+37284 and W+52198.

The period is therefore 29830 (= 14915 APU cycles), and the first flag
after a write comes 29831 cycles later only because of the 3-cycle write
delay. `cpu_interrupts_v2` 5 and `apu_reset` 4017_timing both count
29831-cycle loops against a 29830-cycle sequence to measure a phase, so
both need the period exact.

## Changes Made

### 1. Sequencer on the nesdev schedule (src/apu/mod.rs)

`frame_cycle` counts CPU cycles since the last restart and is compared
against the `FRAME_*` constants (the nesdev table in APU cycles, doubled).
The mode bit is latched from the written value when the write lands, not
when it is written, so a `$4017` write does not change what the sequencer
does during the 3-4 cycle delay (apu_test 3 #6). In each `step`:

1. If a `$4017` write lands this cycle, `frame_cycle` is set to 0 and, in
   5-step mode, the quarter and half units clock; the sequencer does not
   otherwise advance this cycle.
2. Otherwise `frame_cycle` advances and the action scheduled for it runs.
3. Length counter reloads and halt writes from the previous cycle land.

`raise_frame_interrupt` runs on 29828, 29829 and 29830 of the 4-step
sequence unless inhibited. Each of the three sets the flag, so a `$4015`
read inside the window is followed by the flag coming back on the next
cycle, which is what apu_test 6 #4 checks.

The 3/4-cycle write delay by `cycles` parity is unchanged from
docs/debugging/BUS_TICK_TIMING.md; which absolute parity gets the shorter
delay is unobservable because `sync_apu` self-calibrates.

### 2. Length counters

A `LengthCounter` struct replaces the per-channel `enabled` and
`length_counter` fields:

- `$4015` disable zeroes the counter and drops any pending reload;
  `load` is ignored while disabled (apu_test 1 #6/#7, apu_reset
  4015_cleared #2, works_immediately).
- The halt flag (`$4000`/`$4004`/`$400C` bit 5, `$4008` bit 7) suspends
  clocking (apu_test 1 #8). The triangle's bit is also its linear counter
  control flag, which the channel did not store at all; `clock_linear_counter`
  now clears the reload flag when control is clear, so a triangle note
  with control clear decays instead of reloading forever.
- A reload or halt write lands one cycle after the write, after that
  cycle's frame clock. If the counter was clocked in between (it changed and
  was not already 0) the reload is dropped. This follows blargg's older
  `10.len_reload_timing` readme ("reload during length clock when ctr > 0
  should be ignored") and Mesen; none of the ROMs in this repository
  exercise the race, so it is covered by a unit test only.

### 3. Power-up and reset

Power-up (`Apu::new`) is a `$4017 = $00` write with the 3-cycle delay
pending: the 4-step sequence starts with the IRQ enabled, and `$4015`
reads 0.

Soft reset (`Apu::reset`, called from `System::reset` before the CPU's
7-cycle reset sequence) now does only what apu_reset can observe:

- `$4015` cleared: channels disabled, length counters and DMC sample
  stopped, DMC IRQ flag clear.
- The last `$4017` value is written again with the IRQ inhibit bit dropped
  (4017_written #3 checks the mode survives; the readme notes inhibit "is
  sometimes cleared"). The frame IRQ flag is clear.
- Everything else is left alone. len_ctrs_enabled #3 loads a length into
  a triangle whose control flag was set before the reset and expects it to
  still be halted afterwards; the old `reset` rebuilt every channel.
- The triangle sequencer phase restarts (nesdev power/reset notes).

No change to `src/system.rs` was needed.

### 4. 4017_timing delay

The ROM prints "Delay after effective $4017 write: N" and accepts
6 <= N < 13 (hardware is usually 9). It reports 8 here: the sequencer
restarts 3 cycles into the reset, the CPU spends 7 cycles on the reset
vector, and the ROM's own `delay 29812` plus its read lands the first
`$4015` read at cycle 29825, three before the first flag cycle 29828.
The old divider fired at 29828 from cycle 0 and reported 13 (too early).

## Verification

- `apu_test` 1-8 and the combined ROM pass; `apu_reset` all six pass
  (with the 0x81 reset request); `cpu_interrupts_v2` 5 and the combined
  ROM pass.
- New unit tests in src/apu/mod.rs: the three-cycle flag window at
  W+29831..W+29833, the 29830 period after a 29831 (even write) or 29832
  (odd write) first frame, the length clock schedule in both modes,
  enable/halt behaviour, the reload/halt ordering, power-up and reset state.
- nestest still matches all 8991 lines; `cpu_interrupts_v2` 1-4 pass.
- 60 of 76 blargg suites pass (was 49); the 16 still ignored are all PPU
  sprite evaluation or MMC3 revision suites.

## Still open

- Envelope and pitch behaviour in real games needs a human listen; the
  tests only cover what the CPU can read back and the emulator's own
  channel state.
- The DMC memory reader still bypasses the bus and does not stall the CPU.

## Sweep units (issue #21)

`Apu::clock_sweeps` was empty, so `$4001`/`$4005` were stored and never
acted on and every pulse note played flat. Each `Pulse` now carries a
sweep unit following nesdev "APU Sweep":

- `$4001`/`$4005` write the enable flag (bit 7), divider period (bits 6-4),
  negate flag (bit 3) and shift count (bits 2-0), and set the sweep's
  reload flag. Nothing else changes on the write; the divider is only
  reloaded on the next half-frame clock.
- `Pulse::sweep_target` is `period + (period >> shift)` in add mode and
  `period - (period >> shift)` in negate mode. Pulse 1's adder is one's
  complement, so it subtracts one more than pulse 2 (`0x100` with shift 1
  goes to `0x7F` on pulse 1 and `0x80` on pulse 2). A shift of 0 makes the
  change equal to the whole period. The subtraction saturates at 0; the
  addition is not clamped to 11 bits because an out-of-range target is
  what the muting rule looks at.
- `Pulse::clock_sweep` runs on every half-frame clock (both sequencer modes,
  and the immediate clock when a 5-step `$4017` write lands). If the divider
  is 0, the unit is enabled, the shift count is non-zero and the channel is
  not muted, the period becomes the target. Then, if the divider is 0 or
  the reload flag is set, the divider is reloaded from the register value
  and the flag cleared; otherwise it decrements. The check uses the divider
  value before the reload, so the first clock after a `$4001` write with the
  divider already at 0 updates the period at once. `timer_counter` is not
  touched; the new period takes effect when the timer next wraps.
- `$4002`/`$4003` and `$4006`/`$4007` keep writing `timer_period` as
  before, and that is the period the sweep reads and rewrites.

### Muting rule

`Pulse::sweep_muted` is true when the current period is below 8 or the
target period is above `0x7FF`. It is checked continuously in the output
path, not only on sweep clocks, and it ignores the enable flag and the
shift count: a disabled sweep with a period of 7 is still silent, and a
period of `0x700` with add mode and shift 1 (target `0xA80`) is silent even
with bit 7 clear. Muting also stops the period from being updated, so a
sweep-down ends with the period parked just below 8 rather than running to
0. The old output path only checked `period < 8`.

Sweep registers are not reset by `Apu::reset`; like the other channel
registers they are state the reset line leaves alone.

### Verification

Unit tests in src/apu/mod.rs, clocking half-frames with 5-step `$4017`
writes as the length counter tests do:

- `sweep_target_period_both_negate_modes`: add and negate targets on both
  channels, the pulse 1 extra decrement, shift 0 and saturation at 0.
- `sweep_mutes_on_low_period_and_high_target`: output is 15 at period
  `0x100`, 0 at period 7, 15 again at 8, 0 at `0x700` with an out-of-range
  add target while the sweep is disabled, 15 once negate brings the target
  back in range, and a muted channel's period does not change on a clock.
- `sweep_down_sequence_on_half_frame_clocks`: `0x100` with negate and
  shift 1 goes `0x7F, 0x3F, 0x1F, 0x0F, 0x07` on pulse 1 and
  `0x80, 0x40, 0x20, 0x10` on pulse 2, then stops once muted.
- `sweep_divider_reload`: with divider period 2 the period changes on
  clocks 1 and 4, a mid-count `$4001` write reloads the divider on the next
  clock without changing the period, and shift 0 never updates it.

The blargg suites only read `$4015`, so they cannot see any of this; they
stay green (apu_test 1-8, apu_reset, cpu_interrupts_v2, nestest). Pitch
bends in a real game (Mega Man, Contra weapon sounds) still need a human
listen.
