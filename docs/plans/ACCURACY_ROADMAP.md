# NES Emulator Accuracy Roadmap

**Date:** 2026-09-04
**Type:** Architecture Plan
**Status:** Phases 1-5 complete (2026-09-05); remaining: apu_test 1/5/6 and cpu_interrupts_v2 5 (APU frame counter timing)
**Tracking:** GitHub issues #1 through #4 one per phase; Phase 5 is #11-#14; #10 is the interrupt-sampling follow-up

## Executive Summary

The emulator renders some games but fails or misbehaves across most of the
commercial library. An audit of the current code (v0.1.0) found that the
failures are structural, not per-game bugs. Three gaps account for almost all
of the observed problems:

1. **Nothing verifies accuracy.** There are no unit tests and no test ROMs.
   Every fix has been judged by eye against Super Mario Bros, so fixes regress
   each other.
2. **CHR bank switching never reaches the PPU.** The first 8 KB of CHR ROM is
   copied into PPU VRAM once at load time. Mapper CHR bank writes change
   cartridge state that nothing reads.
3. **The CPU has no IRQ line.** Only NMI is polled. The MMC3 module in
   `src/cartridge/mapper4.rs` is dead code: nothing references it, clocks its
   counter, or consumes its pending flag. APU frame and DMC IRQs are never
   delivered either.

This plan defines "compliant" as a concrete, testable bar and lays out phases
that reach it in dependency order.

## Definition of Compliant

The emulator is considered accurate when all of the following pass headlessly
under `cargo test`:

| Suite | What it proves |
|-------|----------------|
| nestest (golden log compare from PC $C000) | Every official and unofficial opcode, flags, cycle counts |
| blargg `instr_test-v5`, `cpu_timing_test6`, `cpu_interrupts_v2` | CPU behaviour, timing, NMI/IRQ polling |
| blargg `ppu_vbl_nmi`, `ppu_open_bus`, `sprite_hit_tests_2005.10.05`, `sprite_overflow_tests` | PPU vblank/NMI timing, open bus, sprite 0 hit, overflow |
| blargg `apu_test`, `apu_reset` | APU frame counter, length counters, IRQ |
| `mmc3_test_2` | MMC3 IRQ counter and A12 clocking |
| `oam_read`, `oam_stress` | OAM DMA and $2004 behaviour |

Sub-cycle, transistor-level fidelity is explicitly out of scope. Passing the
suites above is what mainstream emulators (Mesen, puNES, Nestopia) treat as
the compatibility bar for the commercial library.

## Current Architecture (as audited)

| Area | State | Problem |
|------|-------|---------|
| CPU | Inline `match` in `src/system.rs`, ~2200 lines, instruction-level timing | Returns a cycle count; PPU/APU catch up after the instruction |
| PPU | `src/ppu/mod.rs`, shift-register pipeline (v0.1.0) | Owns its own copy of CHR; sprite eval/fetch batched at cycles 65 and 257 (per-dot since #11) |
| APU | `src/apu/mod.rs` | Stepped once per instruction, not per CPU cycle; no IRQ output |
| Cartridge | Numeric `match` on mapper id in `src/cartridge/mod.rs` | MMC1, MMC3, MMC5, 65 in different styles; `mapper4.rs` unreferenced |
| Interrupts | NMI polled after each instruction | No IRQ line, no edge/level semantics |
| Tests | None | `roms/` contains only commercial games |

## Phases

Each phase is one GitHub issue. Phases 1-3 are independent of each other and
can be worked in parallel. Phase 4 depends on 1 (it cannot be validated
otherwise). Phase 5 depends on 4.

### Phase 1: Headless test-ROM harness (#1)

**Goal:** Make accuracy measurable before changing anything else.

- Add `test-roms/` with nestest, the blargg suites, and `mmc3_test_2`
  (all freely redistributable; document sources in `test-roms/README.md`).
- Add a `tests/` integration harness:
  - Load ROM, run N frames or until a status condition.
  - Blargg convention: $6000 is 0x80 while running, 0x00 on pass, other
    values are failure codes; $6001-$6003 hold the signature DE B0 61;
    $6004 onward is a NUL-terminated result message.
  - nestest: run from PC $C000 with P = 0x24, emit a trace line per
    instruction in Nintendulator format, diff against `nestest.log`.
- Add a `trace` feature or debug flag so the CPU can emit the trace format.
- Mark suites that are expected to fail today with `#[ignore]` and a note,
  so the harness lands green and each later phase un-ignores its suites.

**Done when:** `cargo test` runs the harness, nestest compares at least the
first N lines, and the expected-failure list is documented.

### Phase 2: Mapper trait and PPU CHR routing (#2)

**Goal:** Bank switching affects rendering.

- Introduce a `Mapper` trait: `cpu_read`, `cpu_write`, `ppu_read`,
  `ppu_write`, `mirroring`, `irq_pending`, `clear_irq`, plus an optional
  `ppu_a12_rise` hook for MMC3.
- Move mapper 0, 1, 3, 4, 5, 65 into their own files implementing the trait.
  Wire the existing `mapper4.rs` in rather than the inline MMC3 code.
- Remove the load-time CHR copy from `System::load_cartridge`. Route every
  PPU pattern-table fetch ($0000-$1FFF) through `Mapper::ppu_read`.
- Nametable mirroring is asked of the mapper each access so MMC1/MMC3
  mirroring writes take effect immediately.
- CHR RAM boards write through `Mapper::ppu_write`.

**Done when:** Zelda, SMB3, TMNT, and Final Fantasy render correct tiles
after their first CHR bank switch; MMC1 and MMC3 CHR sections of the blargg
suites pass.

### Phase 3: Interrupt line, IRQ and NMI edge (#3)

**Goal:** Deliver every interrupt source with correct polling semantics.

- Add an IRQ level input to the CPU, OR'd from: mapper `irq_pending`, APU
  frame IRQ, DMC IRQ.
- Poll interrupts at the end of each instruction: NMI is edge-triggered and
  takes priority; IRQ is level-triggered and gated by the I flag.
- Implement APU frame counter IRQ (4-step mode, inhibit flag, $4015 read
  clears it) and DMC IRQ.
- Clock the MMC3 counter. First step: once per scanline at PPU cycle 260
  when rendering is enabled. Second step (Phase 5): on PPU A12 rising edges.
- BRK and IRQ share the vector but set the B flag differently; verify with
  `cpu_interrupts_v2`.

**Done when:** SMB3 and TMNT status bars are stable; `cpu_interrupts_v2`
and `mmc3_test_2` tests 1-4 pass.

### Phase 4: Bus-tick timing, PPU and APU per CPU cycle (#4)

**Goal:** Register reads and writes observe PPU/APU state at the correct
cycle.

- Change `read_byte` / `write_byte` to tick the PPU three times and the APU
  once per call. Remove the post-instruction catch-up loop.
- Audit each instruction so it performs the exact number of bus accesses
  the 6502 does, including dummy reads on page-crossing indexed reads,
  read-modify-write double writes, and the extra cycles on branches taken.
- OAM DMA: 513 or 514 cycles depending on odd/even CPU cycle, executed as
  actual bus reads and $2004 writes.
- Drive the frame loop from the PPU frame counter only; drop the fixed
  29780-cycle budget.
- Audio sampling should be driven from APU cycles, not instruction counts.

**Done when:** nestest passes fully including cycle column;
`cpu_timing_test6`, `ppu_vbl_nmi`, and `apu_test` pass.

### Phase 5: PPU cycle-level details (#5, split into #11 sprite eval, #12 vblank/NMI timing, #13 open bus, #14 OAM; A12 clocking landed with #3)

**Goal:** Close the remaining PPU accuracy gaps.

- Spread sprite evaluation across cycles 65-256 and sprite fetches across
  257-320, replacing the batched calls.
- Sprite overflow flag with the hardware's buggy diagonal scan.
- Sprite 0 hit at the exact pixel, with left-8-pixel clipping rules.
- Odd-frame cycle skip when rendering is enabled.
- PPU open bus decay on $2002 low bits, $2004, and $2007.
- MMC3 IRQ from A12 rising edges rather than a scanline approximation.

**Done when:** `sprite_hit_tests_2005.10.05`, `sprite_overflow_tests`,
`ppu_open_bus`, `oam_read`, `oam_stress`, and all of `mmc3_test_2` pass.
Met with #11 (2026-09-05, docs/debugging/PPU_SPRITE_PIPELINE.md); `mmc3_test_2`
6 is the alternate MMC3 revision and is exclusive with 5 by design.

## Risks and Notes

- Phase 4 is the largest change. Ticking inside the bus functions is far
  cheaper than a micro-op CPU rewrite and reaches the same observable
  accuracy for test ROMs; a rewrite is not planned.
- The uncommitted attribute-shifter and audio diffs on `main` cannot be
  validated until Phase 1 lands. Commit them separately and re-verify once
  the harness exists.
- Commercial ROMs in `roms/` stay out of the test harness; only
  redistributable test ROMs are committed.

## References

- nesdev wiki: CPU interrupts, PPU rendering, PPU frame timing, MMC3
- blargg test ROM documentation (bundled readme per suite)
- Nintendulator nestest.log
- Existing project docs: `docs/debugging/PPU_CYCLE_ACCURATE_REFACTOR.md`
