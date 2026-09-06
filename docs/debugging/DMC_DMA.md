# DMC DMA: CPU Stall, Repeated Reads and the OAM DMA Overlap

**Date:** 2026-09-05
**Type:** Accuracy Fix
**Issue:** #27
**Status:** Complete

## Executive Summary

The DMC memory reader used to fetch its sample byte at the end of whichever
instruction found the buffer empty, through `System::peek`, without ticking
the bus or stalling the CPU. It is now a real DMA: the APU raises a request,
the DMA unit halts the CPU on its next read cycle, spends the hardware's
no-operation cycles with the PPU and APU running, reads the byte through
the bus on a get cycle, and the CPU then repeats the read it was halted on.
The repeated read is what makes `$2007` and `$4016` reads lose data on
hardware, and both blargg suites that measure it now pass:
`dmc_dma_during_read4` (5 of 5) and `sprdma_and_dmc_dma` (2 of 2).

## What hardware does (nesdev "DMA")

- DMA units read on **get** cycles and write on **put** cycles, which
  alternate with the APU clock. The CPU/APU phase is random at power-up,
  so which CPU cycle parity is a get is arbitrary; the emulator uses the
  convention OAM DMA has always used (gets on odd `total_cycles`).
- A DMA can only halt the CPU on a **read** cycle. On a write the halt
  fails and is retried next cycle (read-modify-write has 2 consecutive
  writes, an interrupt sequence 3).
- **DMC DMA** = halt cycle, dummy cycle, alignment cycle if the next
  cycle is not a get, then the get: 3 or 4 cycles. There are two kinds:
  - **Load** DMA, after a `$4015` write enables the channel with an empty
    sample buffer: scheduled to halt on the get cycle 3 or 4 cycles after
    the write. Normally 3 cycles.
  - **Reload** DMA, when the output unit empties the buffer: scheduled to
    halt on a put cycle. Normally 4 cycles (halt on put, dummy on get,
    alignment on put, get).
- While halted, the 2A03 keeps driving the interrupted read on every
  no-operation cycle. `$2007` therefore sees 2 or 3 extra reads before the
  real one. The joypad output enables stay asserted across adjacent reads
  of the same register, so the controller sees one clock for the whole
  halt/dummy/alignment set plus one for the resumed read: 2 clocks, which
  is why games read `$4016` until two reads agree.
- **DMC DMA during OAM DMA**: the DMC get takes precedence over the OAM
  get and OAM DMA pauses; it then needs one alignment cycle to get back
  onto a get. 2 extra cycles in the common case, 1 when the DMC halt
  lands on the second-to-last OAM put, 3 when it lands on the last put.

## Changes Made (src/system.rs)

### 1. The request (`DmcDma`, `tick`, `$4015` write)

`Apu::dmc_fetch_address()` is a level (Some while the buffer is empty and
bytes remain), so `System` edge-detects it:

- `tick`, after `apu.step()`: if no request is outstanding and the APU
  wants a byte, a reload request is recorded with
  `attempt = next put cycle >= total_cycles + 1`.
- the `$4015` write arm: if the write left the APU wanting a byte and
  nothing is outstanding, a load request with
  `attempt = next get cycle >= total_cycles + 3` (the write's own cycle
  is `total_cycles`). A write that clears the channel drops any request.

At halt time `dmc_halt_due` re-checks `dmc_fetch_address()` and drops a
request whose sample has since been stopped.

### 2. The stall (`read_byte`, `dmc_dma_stall`)

`read_byte` ticks, then asks `dmc_halt_due()`: a request whose attempt
tick has arrived halts the CPU on this read cycle. `dmc_dma_stall`:

```
halt cycle      (the tick that just ran)  repeat the CPU's read
dummy cycle     tick                       repeat the CPU's read
alignment cycle tick, only if the next cycle is not a get; repeat the read
get             tick, bus_read(sample address), apu.dmc_supply_sample
resume          tick; the caller's bus_read(addr) is the repeated read
```

`repeat_halted_read` performs a full `bus_read` for every address except
`$4016`/`$4017`, where only the halt cycle's read is performed (one
contiguous set, one controller clock). Everything goes through `bus_read`,
not `read_byte`, so a stall cannot nest. Writes never call into the DMA
path, so a request that lands on a write waits for the next read, as on
hardware.

### 3. Padded cycles are halt-eligible (`cpu_step`)

Most opcode arms make fewer bus accesses than the 6502 does and `cpu_step`
pads the difference at the end (docs/debugging/BUS_TICK_TIMING.md). Those
padded cycles are reads on hardware (dummy reads, internal cycles), so a
DMA attempt that lands on one must halt there; otherwise a reload attempt
scheduled on a put cycle slips to the following get and the stall is 3
cycles instead of 4. This was the first failure mode: blargg's `sync_dmc`
loop is 3421 cycles of code plus "4 DMC wait-states" against a 3424-cycle
sample period, and with a 3-cycle stall it never drifts and the ROM hangs.

The padding loop now offers each padded tick to `dmc_halt_due` and runs
`dmc_dma_stall(None)` (no repeated read: the dummy read's address is not
modelled). The one exception is read-modify-write opcodes
(`is_read_modify_write`), whose padded cycle stands for the dummy write of
the unmodified value; `padding_is_write` keeps the DMA off it.

### 4. DMA cycles stretch the instruction (`dma_cycles`)

The old `dma_in_instr` flag only relaxed the "accesses <= declared"
assertion. With a stall in the middle of an instruction that has padded
cycles (a NOP halted on its opcode fetch), padding to `declared` swallowed
the padded cycle, so the instruction came out one cycle short. `cpu_step`
now pads to `declared + dma_cycles`, where OAM DMA and DMC DMA add the
cycles they inserted. This also fixes OAM DMA started by an instruction
with padded cycles.

### 5. Interrupt sample tick (`dmc_stall`, `take_interrupt_snapshot`)

The boundary poll samples the inputs captured at the instruction's
penultimate tick (docs/debugging/INTERRUPT_LINE.md). A stall that halted
the CPU at or before that tick pushes the real penultimate cycle later by
the stall length, since the halted CPU neither polls nor progresses and
re-runs the interrupted cycle afterwards. `dmc_stall` records the halt
tick and length; the snapshot adds the length when `halt_tick <= sample
tick`. A stall on the last cycle (`LDA $2007` halted on its read) leaves
the sample tick alone.

### 6. OAM DMA overlap (`oam_dma`)

The `$4014` write now calls `oam_dma`, which drives the transfer with raw
`tick` + `bus_read`/`bus_write` (so the plain 4-cycle stall in `read_byte`
cannot fire inside it) and checks the DMC request after every tick:

- inside OAM DMA the CPU is already halted, so the DMC halt succeeds on
  any cycle at or after its attempt tick (`dmc_halt_during_oam`);
- the DMC get is owed on the first get cycle at least two cycles after the
  halt (`dmc_get_due_next_cycle`); it takes the get in place of the OAM
  read, and the loop head then inserts the OAM alignment cycle because the
  next cycle is a put;
- a get still owed when the 256th write is done is performed after the
  transfer.

That one rule reproduces every nesdev example: +2 mid-transfer, +1 on the
second-to-last put (the get is the first cycle after the DMA), +3 on the
last put (dummy, alignment, get after the DMA).

### 7. Indexed stores perform their dummy read

`STA abs,X`, `STA abs,Y` and `STA (zp),Y` now read the address before the
page-crossing fix-up on the cycle before the write
(`dummy_read_before_indexed_store`), instead of padding. `read_write_2007`
checks that `STA $2007,X` reads `$2007` and then writes it.

## Debugging steps

1. First run: all five `dmc_dma_during_read4` ROMs showed a blank screen
   after 1800 frames and both `sprdma_and_dmc_dma` ROMs stopped after the
   header. A PC histogram put the CPU in `sync_dmc`'s fine loop.
2. Temporary `eprintln!` traces on the `$4015` write, the `$4015` read and
   the DMC fetch (removed again) showed the loop period at exactly 3424
   cycles instead of 3425: the NOP the halt landed on was losing its padded
   second cycle (fix 4).
3. With that fixed the period was 3425 and the DMA drifted as intended,
   but settled at a steady state with a 3-cycle stall: the attempt tick
   was the padded third cycle of a taken `BNE`, which the DMA could not
   halt on, so it slipped one cycle to a get (fix 3).
4. `read_write_2007` then reported `22 11 22 09 44 55 66 77` for the
   `STA $2007,X` case: no dummy read (fix 7).
5. The unit test for DMC-during-OAM initially measured +3, because the
   reference run started on the other cycle parity (OAM alignment) rather
   than because of the overlap; the test now programs the DMC registers in
   both runs.

## Results

| Suite | Before | After |
|-------|--------|-------|
| dmc_dma_during_read4 dma_2007_read | hangs in sync_dmc | pass (CRC 159A7A8F or 5E3DF9C4) |
| dmc_dma_during_read4 dma_2007_write | hangs in sync_dmc | pass |
| dmc_dma_during_read4 dma_4016_read | hangs in sync_dmc | pass (08 08 07 08 08) |
| dmc_dma_during_read4 double_2007_read | pass (no DMA involved; see note) | pass |
| dmc_dma_during_read4 read_write_2007 | STA abs,X missing its dummy read | pass |
| sprdma_and_dmc_dma | hangs in sync_dmc | pass |
| sprdma_and_dmc_dma_512 | hangs in sync_dmc | pass |
| nestest (registers, CYC, PPU column) | pass | unchanged |
| apu_test 7/8 (DMC), cpu_interrupts_v2, ppu_vbl_nmi, sprite, mmc3 | pass | unchanged |

`double_2007_read` involves no DMA: it reads `$2007` twice on adjacent
cycles through a page-crossing `LDA abs,X`. It passes because indexed
loads do not yet perform their dummy read, so the emulator performs a
single `$2007` read, and the ROM's list of accepted outputs happens to
include the single-read case. A lane that makes `LDA abs,X` bus-exact
will need the PPU's adjacent-cycle `$2007` behaviour for it to keep
passing.

`sprdma_and_dmc_dma` prints the OAM DMA cost for 16 DMC offsets (525-528
cycles) and checks the table's CRC; the `_512` variant runs the DMA from
a different alignment.

## Tests

`src/system/tests.rs`:

- `dmc_dma_halting_a_4016_read_clocks_the_controller_twice`: `LDA $4016`
  with B pressed and a load DMA timed onto the read (the test picks the
  `$4015` write's parity so the halt lands on the fourth cycle); the CPU
  reads the second shifted bit, the instruction takes 4 + 3 cycles, and
  the next read returns the third bit.
- `reload_dma_stalls_four_cycles_and_can_halt_on_a_padded_cycle`: NOPs
  under a 17-byte sample; the load DMA costs 3 cycles, each reload 4, and
  the reloads halt on the NOP's padded second cycle.
- `dmc_dma_inside_oam_dma_costs_two_extra_cycles`: `STA $4014` with a
  request timed into the middle of the transfer costs the reference
  OAM DMA plus 2.

The `Apu` handshake (`dmc_fetch_address` / `dmc_supply_sample`) is
unchanged, so the DMC unit tests in `src/apu/mod.rs` are untouched.

## Not modelled

- The 2A03 register-activation quirk: while halted, a CPU read of
  `$4000-$401F` combines with the DMA address's low 5 bits, so a `$4016`
  read colliding with a DMA from `$xx15`/`$xx16`/`$xx17` can read `$4015`
  or clock the other joypad. The emulator always performs a clean sample
  read.
- The RF Famicom's per-cycle joypad clocking (3 extra reads instead of 1).
- The aborted 1-cycle DMA when a sample is stopped just before a reload
  would schedule, and the late-2A03G/2A03H extra reload.
- Repeated reads on padded cycles: the dummy read's address is not
  modelled there, so a DMC DMA halting on, say, the dummy read of an
  indexed load does not re-read that address. Padded cycles also come at
  the end of the instruction rather than in hardware order, so a halt on
  one of them sits a cycle or two later in the instruction than on
  hardware; the cycle count is exact.
