# CPU Interrupt Line: NMI Edge, IRQ Level, APU Frame and DMC IRQs

**Date:** 2026-09-04
**Type:** Architectural Fix (CPU interrupt machinery, APU IRQ sources)
**Severity:** Critical (games that rely on IRQs could not run)
**Status:** Complete (CPU/APU half 2026-09-04, MMC3 half 2026-09-05)
**Issue:** #3 (Phase 3 of docs/plans/ACCURACY_ROADMAP.md)

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
of each instruction, not at the boundary; that refinement is issue 10.

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

### 6. APU DMC playback and IRQ
**File:** `src/apu/mod.rs`, `Dmc`

The DMC now has a working output unit and memory reader:

- NTSC rate table; the timer ticks once per CPU cycle and shifts one bit per
  period, adjusting `output_level` by +/-2 (so DMC audio is now produced).
- Memory reader uses a request/supply handshake so the APU never needs a
  reference to the CPU bus: `apu.dmc_fetch_address() -> Option<u16>` asks for
  a byte, `System` reads it, `apu.dmc_supply_sample(byte)` delivers it. The
  frame loop does this once per instruction.
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

- **No interrupt hijacking / one-instruction I-flag delay.** Hardware polls
  during the last cycle of an instruction, so CLI/SEI/PLP/RTI changes to I
  take effect one instruction late and a BRK can be hijacked by an NMI.
  `cpu_interrupts_v2` tests 2-5 exercise these; they are Phase 4 material.
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

## References

- nesdev wiki: CPU interrupts, APU Frame Counter, APU DMC, APU $4015
- docs/plans/ACCURACY_ROADMAP.md, Phase 3
- docs/debugging/PPU_CYCLE_ACCURATE_REFACTOR.md (NMI generation in the PPU)
