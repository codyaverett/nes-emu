# CPU Interrupt Line: NMI Edge, IRQ Level, APU Frame and DMC IRQs

**Date:** 2026-09-04
**Type:** Architectural Fix (CPU interrupt machinery, APU IRQ sources)
**Severity:** Critical (games that rely on IRQs could not run)
**Status:** Complete (CPU/APU half 2026-09-04, MMC3 half 2026-09-05, sampling point 2026-09-05)
**Issue:** #3 (Phase 3 of docs/plans/ACCURACY_ROADMAP.md); sampling point #10

## Executive Summary

The CPU had no IRQ input at all, and NMI was handled ad hoc in the frame loop.
This change adds a proper interrupt poll at the instruction boundary with
edge-triggered NMI and level-triggered IRQ, an IRQ line that ORs every source,
a correct `irq()` sequence, and working APU frame-counter and DMC interrupt
flags. The mapper (MMC3) contribution to the line is stubbed as a plain field
so a follow-up can feed it without touching the CPU again.

## Background

### What was missing

1. **No IRQ path.** `run_frame_with_audio` checked `ppu.nmi_interrupt` after
   each instruction and called `nmi()`. Nothing ever looked at the APU's
   `frame_interrupt` or `dmc.interrupt` flags, and the MMC3 `irq_pending`
   flag in the cartridge was never consumed. Any game that programs the APU
   frame IRQ, the DMC IRQ, or the MMC3 scanline counter would set up a handler
   and wait forever.
2. **APU frame counter ran at the wrong rate.** `apu.step()` was called once
   per *instruction*, not once per CPU cycle, so the frame sequencer fired
   every 7457 instructions (roughly 3x too slow) and the channel timers ran at
   the wrong pitch.
3. **$4017 side effects missing.** Writing the frame counter register did not
   reset the sequencer, and 5-step mode did not clock the quarter/half-frame
   units immediately.
4. **DMC never finished a sample.** `bytes_remaining` was set by `$4013` but
   never decremented; there was no memory reader, so the DMC interrupt flag
   could never be raised.
5. **BRK leaked the B flag.** The BRK arm did `cpu_status |= 0x10` on the live
   status register. B is only meaningful in the copy of P pushed to the stack;
   leaving it set meant an NMI arriving inside a BRK handler (before RTI/PLP
   cleared it) pushed P with B set, which a handler could misread as a BRK.

## Changes Made

### 1. Interrupt state on `System`
**File:** `src/system.rs` (struct fields near line 32)

```rust
/// Latched rising edge of the PPU NMI output.
nmi_pending: bool,
/// Mapper contribution to the IRQ line (level). Always false for now.
pub mapper_irq: bool,
```

Both are initialised in `new()` and cleared in `reset()`.

### 2. The polling point
**File:** `src/system.rs`, top of `cpu_step`

```rust
let declared = match self.poll_interrupts() {
    Some(cycles) => cycles as u16,
    None => self.execute_opcode() as u16,
};
```

The poll runs at the boundary between the previous instruction and the next
opcode fetch. Since the bus-tick change (docs/debugging/BUS_TICK_TIMING.md,
issue 4) every cycle of the previous instruction has already been ticked by
the time `cpu_step` is entered again, so the PPU NMI latch and the APU and
mapper IRQ levels seen here are the ones at that boundary. An OAM DMA runs to
completion inside the `$4014` write, so interrupts wait out the DMA as on
hardware. (Before issue 4 the frame loop ticked the PPU and APU after
`cpu_step` returned, which is why the poll was placed at the top rather than
the bottom of the function; that reasoning no longer applies but the spot is
still the right one.)

Hardware actually samples the interrupt inputs during the penultimate cycle
of each instruction, not at the boundary; see "Sampling Point" below
(issue 10), which keeps the poll here but has it consume a snapshot.

The interrupt sequence declares 7 cycles; `cpu_step` pads the two cycles that
the five bus accesses (three pushes, two vector reads) do not cover.

### 3. `poll_interrupts`: edge vs level
**File:** `src/system.rs`, `fn poll_interrupts` (near line 2655)

```rust
if self.ppu.nmi_interrupt {
    self.ppu.nmi_interrupt = false;
    self.nmi_pending = true;
}
if self.nmi_pending {
    self.nmi_pending = false;
    self.nmi();
    return Some(7);
}
let irq_line = self.apu.irq_pending() || self.mapper_irq;
if irq_line && (self.cpu_status & 0x04) == 0 {
    self.irq();
    return Some(7);
}
None
```

- **NMI is edge-triggered.** The PPU sets `nmi_interrupt` exactly once per
  rising edge of its NMI output (vblank start with NMI enabled, or NMI enabled
  during vblank in `write_ctrl`). The CPU consumes that latch into
  `nmi_pending` and services it once; no matter how long the output stays
  high, one edge means one NMI. We deliberately do *not* re-derive the level
  from `ctrl.NMI_ENABLE && status.VBLANK_STARTED` at poll time: an instruction
  that reads `$2002` between the PPU tick and the poll would clear the vblank
  flag and erase the edge, turning a 2-7 cycle window into an NMI-suppression
  window (hardware only suppresses a same-dot read).
- **IRQ is level-triggered.** The line is the OR of every source and is
  serviced whenever it is high and the I flag is clear. Sources hold their
  flag until acknowledged (`$4015` read for the frame IRQ, `$4015` write for
  the DMC IRQ, `$E000` write for MMC3), so a handler that never acknowledges
  its source is re-entered immediately after RTI, as on hardware.
- **NMI has priority over IRQ.** An NMI sets I, so a simultaneously pending
  IRQ waits until the NMI handler clears I or executes RTI.

### 4. `irq()` and the B flag
**File:** `src/system.rs`, `fn nmi` / `fn irq` (near line 2676)

| Sequence | Pushed P bits 5:4 | Vector | Sets I |
|----------|-------------------|--------|--------|
| BRK      | `11` (B set)      | $FFFE  | yes    |
| IRQ      | `10` (B clear)    | $FFFE  | yes    |
| NMI      | `10` (B clear)    | $FFFA  | yes    |

Both `nmi()` and `irq()` push `(cpu_status & !0x10) | 0x20`. The BRK arm no
longer sets bit 4 in the live register; it only ORs `0x30` into the pushed
copy.

### 5. APU frame counter IRQ
**File:** `src/apu/mod.rs`

- `frame_divider` (new field) counts CPU cycles between sequencer steps,
  separate from `cycles` so a `$4017` write can reset the sequencer without
  disturbing the channel timer parity.
- In 4-step mode the last step sets `frame_interrupt` unless
  `frame_interrupt_inhibit` is set. The flag is held until a `$4015` read or a
  `$4017` write with bit 6 set.
- `$4017` write: stores mode/inhibit, clears the flag if inhibit, resets
  `frame_sequence` and `frame_divider`, and in 5-step mode clocks envelopes,
  linear counter, length counters and sweeps immediately.
- `pub fn irq_pending(&self) -> bool` returns `frame_interrupt || dmc.interrupt`.

Superseded by issue 18: the 7457-cycle `frame_divider` is now a
`frame_cycle` counter on the nesdev schedule and the flag is raised on
three consecutive cycles. See docs/debugging/APU_FRAME_COUNTER.md.

### 6. APU DMC playback and IRQ
**File:** `src/apu/mod.rs`, `Dmc`

The DMC now has a working output unit and memory reader:

- NTSC rate table; the timer ticks once per CPU cycle and shifts one bit per
  period, adjusting `output_level` by +/-2 (so DMC audio is now produced).
- Memory reader uses a request/supply handshake so the APU never needs a
  reference to the CPU bus: `apu.dmc_fetch_address() -> Option<u16>` asks for
  a byte, `System` reads it, `apu.dmc_supply_sample(byte)` delivers it.
  Since issue 27 the read is a real DMA that halts the CPU
  (docs/debugging/DMC_DMA.md); before that the frame loop did it once per
  instruction.
- When the byte count reaches zero: loop flag set -> restart the sample; else
  IRQ enabled -> set `dmc.interrupt`.
- `$4015` write: bit 4 set restarts the sample if none is playing, bit 4 clear
  zeroes `bytes_remaining`; any `$4015` write clears the DMC interrupt flag.
- `$4010` write with bit 7 clear also clears the DMC interrupt flag.
- **Hardware note:** a `$4015` read clears the *frame* interrupt flag only.
  The issue text says the read clears the DMC flag as well; nesdev documents
  that it does not, and the implementation follows the hardware.

### 7. APU stepped once per CPU cycle
**File:** `src/system.rs`, frame loop (near line 230)

```rust
for _ in 0..cpu_cycles {
    self.apu.step();
}
```

This is what makes the frame IRQ run at ~240 Hz. It also changes the rate of
every channel timer, so audio pitch changes; this needs a listening pass.

### 8. Tests
- `src/system/tests.rs` (child module of `system`, declared near the `use`
  lines so the tail of `impl System` stays free for other lanes): synthetic
  32 KB NROM ROM with NOP-filled handlers and distinct vectors. Verifies IRQ
  taken when I is clear and the APU frame flag is set, not taken when I is set,
  taken after CLI while the line is held, stops after `$4015` acknowledge,
  `mapper_irq` drives the line, NMI fires once per edge, NMI beats IRQ, and
  BRK pushes B and uses `$FFFE`.
- `src/apu/mod.rs` `mod tests`: frame IRQ set on step 4, held, inhibited and
  cleared by `$4017` bit 6, never set in 5-step mode, sequencer reset on
  `$4017` write, 5-step immediate clock, DMC IRQ set on sample end, not set
  when disabled or looping, cleared by `$4015` write and `$4010` bit 7 clear.

## Known Limitations

- **Interrupt hijacking and the I-flag delay** landed with the sampling
  point (below).
- **No DMC DMA stall.** Each DMC byte fetch steals up to 4 CPU cycles on
  hardware; not modelled.
- **$4017 write delay.** The sequencer reset takes effect 3-4 CPU cycles after
  the write on hardware; here it is immediate. The frame IRQ flag is also set
  on three consecutive cycles on hardware; here it is a single set.
- **Frame step length.** `FRAME_STEP_CYCLES = 7457` approximates the real
  step lengths (7457, 7456, 7458, 7457/7458); the 4-step period is 29828
  instead of 29830 cycles.

## Verification Steps

Headless (all pass):

```
cargo build
cargo test          # 17 tests: 9 in system::tests, 8 in apu::tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Needs a human pass:

- Run a game that uses the APU frame IRQ and confirm it no longer hangs on a
  handler that never fires.
- Confirm Super Mario Bros (NROM, NMI only) has not regressed: title screen,
  scrolling, sprite 0 status bar.
- Listen for audio pitch changes from the per-cycle APU stepping.

## MMC3 Half (landed 2026-09-05)

**Files:** `src/ppu/mod.rs`, `src/cartridge/mapper.rs`, `src/cartridge/mapper4.rs`, `src/system.rs`

The MMC3 counts rising edges of PPU address line A12, not scanlines. The first
attempt clocked the counter once per scanline at PPU cycle 260; that renders
status bars but cannot pass `mmc3_test_2`, whose first three tests toggle A12
by writing `$2006`. So the PPU now models the address bus:

1. `Ppu::drive_addr_bus(addr, mapper)` is called wherever the hardware puts a
   new value on the PPU address bus: every `read_vram`/`write_vram` (tile,
   attribute, pattern and sprite fetches), the second `$2006` write, and the
   post-increment `v` after a `$2007` access.
2. A12 must have been low for at least `A12_FILTER_CYCLES` (8) PPU cycles
   before a rise counts. This mirrors the MMC3 hardware filter and ignores the
   rapid toggling inside a tile fetch when background and sprites use
   different pattern tables. The low-time counter is advanced in `Ppu::step`.
3. A filtered rise calls the new `Mapper::ppu_a12_rise` hook. MMC3 implements
   the counter there (`clock_scanline` remains for MMC5, which really counts
   scanlines).
4. Sprite fetches (cycles 257-320) now run whenever rendering is enabled, not
   only when sprites are shown, and empty slots fetch dummy tile `$FF` from the
   sprite table, as on hardware. The pre-render line performs the same eight
   dummy fetches. Those fetches are what produce the one A12 rise per scanline
   (241 per frame) that games rely on.
5. `System::poll_interrupts` ORs the loaded mapper's `irq_pending()` into the
   IRQ line. `$E000` writes acknowledge, so the level drops when the handler
   does what real code does.

**Results:** `mmc3_test_2` 1, 2, 3 and 5 pass. Test 4 (scanline timing) needs
the sprite fetches spread across cycles 257-320 at their exact positions
rather than batched at 257; that is Phase 5. Test 6 checks the alternate MMC3
revision, which behaves differently from test 5 on reload-to-zero, so an
emulator passes one of the two by design.

## Sampling Point (landed 2026-09-05, issue 10)

**Files:** `src/system.rs`, `src/system/tests.rs`, `tests/blargg.rs`

### What hardware does

The 6502 has an edge detector on NMI and a level detector on IRQ. Both look
at their input during phi2 of every cycle and raise an internal signal that
the instruction logic polls "before the last cycle" of each instruction.
The net effect is one cycle of latency: an input asserted during the
penultimate cycle of an instruction is serviced after that instruction; one
asserted during the last cycle is not seen until the next instruction's
poll, so the next instruction always runs first. On top of that:

- CLI, SEI and PLP write P during their last cycle, after the poll has
  already read the old I flag. The change to I is therefore only effective
  for the poll of the following instruction (the flag itself changes at
  once: CLI then PHP pushes I clear). RTI pulls P earlier and is not delayed.
- BRK and the interrupt sequences do not poll at their end, so at least one
  handler instruction always executes. If an NMI edge was captured by
  `tick` during their cycles 1-4, or was already pending when the sequence
  started (that is, visible to a poll in cycles 1-5), the vector fetched in
  cycles 6-7 is $FFFA instead of $FFFE ("hijacking"); BRK still pushes P
  with B set.
- A taken branch that does not cross a page polls only before its second
  cycle. An interrupt arriving during its last two cycles waits one more
  instruction. A not-taken (2 cycle) or page-crossing (4 cycle) branch
  polls normally.

### How the row counts in the readme pin this down

`test-roms/cpu_interrupts_v2/readme.txt` lists the result for each
one-cycle shift of the interrupt against a fixed instruction stream. Calling
`v` the first cycle whose poll can see the input (the cycle after it was
asserted), test 2 (`CLC BRK SEC`) shows 1 row before CLC, 2 rows after CLC,
5 hijack rows and 2 rows after SEC: `v` in BRK cycles 1-5 hijacks, `v` in
BRK cycles 6-7 is not polled by BRK and lands after SEC. Test 3 (`LDA CLC
[IRQ] SEC`) shows 7 rows of "NMI interrupting IRQ": `v` in CLC cycles 1-2
(NMI wins over the IRQ at the boundary) plus IRQ-sequence cycles 1-5
(hijack), then 2 rows after SEC for cycles 6-7. Test 5 `test_branch_taken`
shows the IRQ with `v` = branch cycle 3 landing after the 4-cycle LDA that
follows (CK 05), where `test_jmp` takes it after the JMP.

### Implementation

Everything stays in `System`; the opcode arms are untouched except for
CLI/SEI/PLP, BRK and `branch_taken`.

1. **Capture per tick.** `tick` calls `sample_interrupt_inputs` after the
   PPU and APU have advanced and *before* the bus access of that tick. It
   moves the PPU edge latch (`ppu.nmi_interrupt`) into `nmi_pending`,
   remembering the tick at which it was first seen (`nmi_seen_tick`), and
   records the IRQ line level (APU OR mapper OR `mapper_irq`) as bit
   `tick - 1` of `irq_hist` (ticks beyond 16 are not recorded; the sample
   tick is never past 7).
2. **Snapshot at the end of the instruction.** `cpu_step` pads to the
   declared cycle count as before, then `take_interrupt_snapshot(declared)`
   reads back the capture at `sample_tick`: `declared - 1`, or 1 when
   `branch_taken` set `poll_tick` for a non-crossing taken branch. It
   produces `sampled_nmi` (NMI pending and seen at or before the sample
   tick) and `sampled_irq` (level high at the sample tick, no NMI, and I
   clear where I is the value CLI/SEI/PLP stashed in `i_flag_for_poll`
   before modifying P, or the live flag otherwise). BRK and the sequences
   set `no_poll`, which forces both to false.
3. **Service at the boundary.** `poll_interrupts` at the top of the next
   `cpu_step` consumes `sampled_nmi`/`sampled_irq`; it never reads the live
   inputs. `nmi_pending` is cleared when the NMI is serviced (or when a
   hijack takes it). An edge not taken by the previous instruction is
   re-based to tick 0 at the start of the next one, so it is taken then.
4. **Sequences.** `nmi()` and `irq()` now perform the two dummy reads of
   cycles 1-2 and BRK reads its padding byte, so all three make exactly
   seven bus accesses in hardware order and the hijack check
   (`brk_or_irq_vector`: `nmi_pending && nmi_seen_tick <= 4`, evaluated
   just before the vector read) lines up with cycle 5.

Why the padding scheme does not get in the way: the sample tick is a tick
index, and ticks 1..declared advance the PPU and APU in the same order as
hardware cycles even when the bus accesses inside them are not in hardware
order. Sampling *before* the access is what makes a padded instruction
equivalent: for a read-modify-write or an indexed store the real access
happens one tick earlier than on hardware (the dummy access is padded at
the end), so the capture at tick `declared - 1`, taken before that tick's
access, sees the same pre-access state hardware sees at its penultimate
cycle. A write to `$2000` that enables NMI during vblank on the last tick
of `STA $2000` is therefore first captured on tick 1 of the next
instruction and serviced after it, which is what `ppu_vbl_nmi` 04 #11
("immediate occurrence should be after NEXT instruction") checks.

OAM DMA runs inside the `$4014` write, which is the last tick of the store;
the store's sample tick precedes it, so an IRQ that rises during the DMA is
taken after the instruction following the store (test 4 rows +11 to +526).

### Results

| Suite | Before | After |
|-------|--------|-------|
| cpu_interrupts_v2 1-cli_latency | #4 one instruction after CLI | pass |
| cpu_interrupts_v2 2-nmi_and_brk | wrong rows | pass |
| cpu_interrupts_v2 3-nmi_and_irq | wrong rows | rows shifted one cycle (see below) |
| cpu_interrupts_v2 4-irq_and_dma | every offset one instruction early | pass |
| cpu_interrupts_v2 5-branch_delays_irq | wrong PC column | PC column matches, CK column wrong (see below) |
| cpu_interrupts_v2 combined | fails in test 1 | fails in test 3 |
| ppu_vbl_nmi 04-nmi_control | #11 | pass |
| nestest | 8991 lines | unchanged (registers, CYC, PPU column) |

No other blargg suite changed; 38 pass, 38 remain ignored.

### Still failing, and why (outside src/system.rs)

**3-nmi_and_irq.** Our output is the expected table shifted one row: row 0
already shows "NMI after LDA #1" (21) where hardware shows "before" (23),
and there are three rows of 25 instead of two. The IRQ column is right (the
IRQ lands after CLC in every row). The test syncs to vblank with
`sync_vbl` (a `$2002` polling loop) and then waits about two frames before
the sequence; test 2 uses the same sync but waits one frame and passes.
Two NTSC frames are 59561.33 CPU cycles, so which cycle sees the second
vblank depends on the sub-cycle PPU/CPU phase and on the exact cycle at
which the vblank flag becomes readable through `$2002` (the sync's
reference). Both are PPU-side: `ppu_vbl_nmi` 02 (vbl_set_time) and 05-08
are the tests for them, tracked under the Phase 5 PPU issues. The
interrupt sampling itself is not implicated because test 2 (same NMI path,
same sync) and test 4 (IRQ path, 527 offsets) are exact.

**5-branch_delays_irq.** The PC column, which is the observable for the
branch rule (which instruction the IRQ was taken after), matches the readme
for `test_jmp`, and the unit test
`taken_branch_without_page_cross_delays_irq_from_its_second_cycle` checks
the special case directly. The CK column is measured by the test's IRQ
handler: it delays 29830 - 13 cycles, then loops `dex; delay 29831 - 13;
bit $4015; bit $4015; bvc` until it observes the APU frame flag, which
gives the IRQ's phase inside the frame period with one-cycle resolution.
That needs the frame IRQ flag to be raised on exactly the three cycles
hardware raises it and a 29830-cycle period; our `Apu` uses a 7457-cycle
step (29828) and a single set, so CK prints a constant 05/06. This is the
same defect `apu_test` 6 (irq_flag_timing, "flag first set too soon")
reports, in `src/apu/mod.rs`.

**Combined ROM.** Stops at the first failing sub-test (3).

### Tests

`src/system/tests.rs` (15 tests): every "arm a source, then step" test now
executes one instruction before the 7-cycle sequence, which is the one
instruction of latency. New: `cli_takes_effect_one_instruction_late`,
`cli_sei_allows_exactly_one_irq_with_i_set_in_pushed_status`,
`plp_delays_i_flag_but_rti_does_not`,
`irq_asserted_on_penultimate_cycle_is_taken_after_that_instruction`,
`irq_asserted_on_last_cycle_waits_one_more_instruction` (both land the APU
frame flag on a chosen tick of a NOP by pre-stepping the APU, since `tick`
performs exactly one `apu.step()`),
`taken_branch_without_page_cross_delays_irq_from_its_second_cycle` (JMP
versus BCC-to-next with the flag on tick 2),
`nmi_during_brk_hijacks_its_vector` and
`nmi_during_irq_sequence_hijacks_its_vector`.

### Debugging notes

- First attempt: the poll consumed the snapshot correctly but every
  existing unit test expecting `cpu_step() == 7` immediately after arming a
  source failed with 2. That is the intended one-instruction latency, not
  a bug; the tests were updated rather than the model.
- Test 5 printed a constant CK of 05/06 both before and after the change,
  which pointed away from the CPU: the handler source (fetched from the
  public nes-test-roms mirror) confirmed CK is an APU frame-flag phase
  measurement.
- Test 4 moved from "every IRQ one instruction early" to exact with no
  DMA changes, confirming the DMA parity cycle was already right.

## References

- nesdev wiki: CPU interrupts (including "Delayed IRQ response after CLI,
  SEI, and PLP", "Interrupt hijacking" and "Branch instructions and
  interrupts"), APU Frame Counter, APU DMC, APU $4015
- test-roms/cpu_interrupts_v2/readme.txt and the test sources in the
  nes-test-roms mirror (cpu_interrupts_v2/source)
- docs/plans/ACCURACY_ROADMAP.md, Phase 3
- docs/debugging/PPU_CYCLE_ACCURATE_REFACTOR.md (NMI generation in the PPU)
