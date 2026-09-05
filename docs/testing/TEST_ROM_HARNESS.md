# Test ROM Harness

**Date:** 2026-09-04
**Tracking:** GitHub issue #1 (Phase 1 of `docs/plans/ACCURACY_ROADMAP.md`)

The harness runs freely redistributable NES test ROMs headlessly under
`cargo test` and asserts on the result each ROM reports. It is the
measuring stick for the accuracy phases: each later phase un-ignores the
suites it fixes, and a regression in an already-passing suite fails CI.

## Layout

| Path | Purpose |
|------|---------|
| `test-roms/` | ROM images, `nestest.log`, per-suite readmes. Sources in `test-roms/README.md`. |
| `tests/common/mod.rs` | Shared runner: ROM loading, `$6000` protocol, legacy `$F8` protocol, nametable text decoding. |
| `tests/blargg.rs` | One `#[test]` per blargg ROM (single and combined). |
| `tests/nestest.rs` | nestest golden-log comparison and result-byte check. |
| `src/system.rs` (tail of `impl System`) | Debug API used by the harness: `peek`, `poke`, register accessors, `step_instruction`, `total_cpu_cycles`, `trace_line`. |

## Running

```sh
cargo test                         # everything that is expected to pass (the CI gate)
cargo test --test blargg           # just the blargg suites
cargo test --test nestest          # just nestest
cargo test -- --include-ignored    # also run the suites known to fail
cargo test --test blargg -- --include-ignored ppu_vbl_nmi   # one family, incl. ignored
cargo test --release -- --include-ignored                    # faster when running everything
```

Ignored tests still compile and can be run individually by name, so a
phase that is working on, say, `ppu_vbl_nmi_01_vbl_basics` can iterate
with:

```sh
cargo test --test blargg ppu_vbl_nmi_01_vbl_basics -- --include-ignored --nocapture
```

The whole blargg file (including ignored tests) takes about 20 s in debug
mode on an M-series laptop. The green subset takes a few seconds.

## What a failure looks like

A `$6000`-protocol failure prints the status byte, the NUL-terminated
message the ROM wrote at `$6004`, and the on-screen console text:

```
apu_test/rom_singles/3-irq_flag.nes failed: status=0x04 after 27 frames (0 resets)
--- $6004 message ---
Flag should be set in $4017 mode $00
3-irq_flag
Failed #4
--- screen ---
 Flag should be set in $4017
 mode $00
```

A nestest failure prints the first log line that disagrees:

```
nestest register mismatch at log line 3640
  expected: DBCF  10 12     BPL $DBE3   A:66 X:00 Y:AA P:E5 SP:FB PPU: 91,160 CYC:10397
  actual:   DBCE  33 10    *RLA ($10),Y A:66 X:00 Y:00 P:67 SP:FB PPU: 91,133 CYC:10395
```

`status=0x80 after N frames` means the ROM never finished inside its
frame budget (it is usually stuck in a PPU sync loop waiting for timing
the emulator does not reproduce); the reason strings call this "hangs".

## Un-ignoring a suite

1. Run the test with `--include-ignored` and confirm it passes on its
   own and under `cargo test -- --include-ignored` (some suites share
   state assumptions, e.g. the sprite overflow ROMs must pass in order).
2. Delete the `ignore = "..."` argument from the `blargg_test!` /
   `legacy_test!` / `screen_test!` invocation in `tests/blargg.rs`, or the
   `#[ignore = ...]` attribute in `tests/nestest.rs`.
3. Update the table below and the phase's "Done when" line in
   `docs/plans/ACCURACY_ROADMAP.md`.
4. If a frame budget (`SHORT`/`MEDIUM`/`LONG` in `tests/blargg.rs`) was
   too tight for a now-working ROM, raise it in the test rather than
   globally.

When a suite that used to pass starts failing, the reason string format
in the ignore attribute is `"<what the ROM reported>; <phase expected to
fix it>"`, so a bisect can be done from the test list alone.

## Reporting protocols handled

| Protocol | Detection | Pass condition |
|----------|-----------|----------------|
| `$6000` (blargg) | Signature `DE B0 61` at `$6001-$6003`, then `$6000 != 0x80` | `$6000 == 0x00`. `0x81` triggers a 10-frame delay, `System::reset()`, and one more frame before re-checking. |
| Zero page `$F8` (2005 sprite tests) | CPU parked on `jmp *` or a KIL opcode | `$F8 == 1` |
| Screen only (`cpu_timing_test6`) | Same halt detection | Nametable 0 contains `PASSED` |
| nestest | Instruction stepping with `trace_line` | Parsed PC/A/X/Y/P/SP (and CYC) equal for every log line |

## Current status (v0.1.0, 2026-09-04)

Legend: pass = runs green in `cargo test`; ignored = known failure, still
compiled and runnable with `--include-ignored`.

### nestest

| Test | Status | Detail |
|------|--------|--------|
| `nestest_trace_format_matches_log_layout` | pass | Trace format pinned against log line 1 |
| `nestest_registers_match_log_prefix` | pass | Lines 1-3639 match on PC/A/X/Y/P/SP |
| `nestest_cycles_match_log_prefix` | pass | Lines 1-3639 also match on CYC |
| `nestest_registers_match_log` | pass | All 8991 lines match on PC/A/X/Y/P/SP |
| `nestest_cycles_match_log` | pass | All 8991 lines match on CYC |
| `nestest_ppu_position_matches_log` | pass | All 8991 lines match on the PPU scanline/dot column |
| `nestest_result_bytes_are_clear` | pass | `$02` and `$03` are both zero at the end of the run |

### CPU bugs found by the harness (fixed, issue 9)

The first harness run found that `cpu_step` had no arm for opcode `$B4`
(`LDY zp,X`), which blocked nestest at line 3640 and four blargg CPU
suites. Fixing it exposed four more cycle-count bugs that the full
nestest cycle column and `cpu_timing_test6` then caught: unofficial
`NOP abs,X`, `LAX abs,Y` and `LAX (ind),Y` ignored the page-crossing
cycle, taken branches ignored the page-crossing cycle, and `SHY`/`SHX`
did not apply the page-crossing address quirk. All 256 opcodes now have
arms, so the unimplemented-opcode fallback was removed.

### blargg suites

| Suite | Pass | Ignored | Ignored tests (reason) |
|-------|------|---------|------------------------|
| instr_test-v5 (16 singles + 2 combined) | 18 | 0 | |
| cpu_timing_test6 | 1 | 0 | |
| cpu_interrupts_v2 (5 + 1) | 6 | 0 | 1, 2 and 4 pass since issue 10, 3 since issue 12, 5 and the combined ROM since issue 18 (exact APU frame-flag period) |
| ppu_vbl_nmi (10 + 1) | 11 | 0 | all pass since issue 12 (NMI line withdrawal, sample dot, odd-frame skip) |
| ppu_open_bus | 1 | 0 | passes since issues 13 (decay latch) and 14 (attribute masking) |
| sprite_hit_tests_2005.10.05 (11) | 11 | 0 | 11 since Phase 4; 01-10 since issue 11 (per-dot sprite pipeline, 8-pixel background shift fix) |
| sprite_overflow_tests (5) | 5 | 0 | 5.Emulator since Phase 4; 1-4 since issue 11 |
| apu_test (8 + 1) | 9 | 0 | 2, 3, 4, 7, 8 pass since the IRQ line landed (Phase 3); 1, 5, 6 and the combined ROM since issue 18 (nesdev sequencer schedule, length counter enable/halt) |
| apu_reset (6) | 6 | 0 | all pass since issue 18 (power-up as a $4017 = $00 write, reset re-writes $4017 and keeps the triangle control flag); 4017_timing reports a delay of 8 |
| oam_read | 1 | 0 | |
| oam_stress | 1 | 0 | passes since attribute bits 2-4 are masked (issue 14) |
| mmc3_test_2 (6) | 5 | 1 | 4 since issue 11 (address bus driven one dot ahead of each fetch, 10-dot A12 filter); 6 is the alternate revision, exclusive with 5 |
| **Total blargg** | **75** | **1** | |

Combined with nestest (7 tests): 82 integration tests pass, 1 is ignored,
plus the ignored game_frames fingerprint run.

### Expected progression

| Phase | Suites expected to flip to pass |
|-------|---------------------------------|
| CPU fix for `$B4` (small, can ride with any phase) | instr_test-v5 05/official_only/all_instrs, nestest full register compare, likely `cpu_timing_test6` moves to a timing failure |
| Phase 3 (IRQ line, NMI edge) | cpu_interrupts_v2, apu_reset (landed: apu_test 2/3/4/7/8 and mmc3_test_2 1/2/3/5 pass) |
| Phase 4 (bus-tick timing) | landed: ppu_vbl_nmi 01/03 and sprite_hit 11 now pass |
| Phase 5 (PPU cycle detail) | complete; landed: oam_stress via issue 14, ppu_open_bus via issues 13 and 14, ppu_vbl_nmi and cpu_interrupts_v2 3 via issue 12, apu_test 1/5/6, apu_reset and cpu_interrupts_v2 5 via issue 18, sprite_hit_tests, sprite_overflow_tests and mmc3_test_2 4 via issue 11 |
| Interrupt sampling (issue 10) | landed: cpu_interrupts_v2 1/2/4 and ppu_vbl_nmi 04 now pass |

## Debug API reference (`src/system.rs`)

| Method | Notes |
|--------|-------|
| `peek(addr) -> u8`, `peek_word(addr) -> u16` | Side-effect free. RAM, PRG RAM, PRG ROM only; `$2000-$5FFF` returns 0 because those registers have read side effects. |
| `poke(addr, value)` | Writes RAM or PRG RAM only. |
| `pc()`, `set_pc()`, `reg_a/x/y/sp/p()`, `set_reg_*()` | CPU register access. |
| `total_cpu_cycles()`, `set_total_cpu_cycles()` | Monotonic cycle counter (separate from the per-frame budget). |
| `step_instruction() -> u32` | One instruction plus its PPU (x3) and APU catch-up and NMI poll; drains a pending OAM DMA stall first. |
| `trace_line() -> String` | Nintendulator-format line for the instruction at PC, without executing it. Register and CYC columns are exact; disassembly omits the `= value` annotations. |
| `instruction_length(addr)` | Byte length from the addressing-mode table. |
| `oam_dma_pending()` | True while a `$4014` stall is outstanding. |
