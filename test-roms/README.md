# Test ROMs

Freely redistributable NES test ROMs used by the headless harness in
`tests/` (see `docs/testing/TEST_ROM_HARNESS.md`). Nothing in this
directory is a commercial game; the project's `roms/` directory is
gitignored and never referenced by the test suite.

## Source

All files were copied from the public collection at
<https://github.com/christopherpow/nes-test-roms> (commit
`95d8f621ae55cee0d09b91519a8989ae0e64753b`, branch `master`). Only the
`.nes` images, `nestest.log`, and each suite's `readme.txt` were taken;
assembly sources were left out (they are in the upstream repository).

The upstream repository carries no top-level licence file. Each suite's
terms are whatever its author stated in the bundled readme; the individual
files below record the author and the readme that accompanies them.

| Directory | Author | What it tests | Reporting protocol |
|-----------|--------|---------------|--------------------|
| `nestest/` | Kevin Horton (ROM), Nintendulator golden log | Every official and unofficial 6502 opcode, flags, cycle counts | Trace compare against `nestest.log`; `$02`/`$03` result bytes in automation mode |
| `instr_test-v5/` | Shay Green (blargg) | CPU instruction behaviour, all addressing modes, official + unofficial | `$6000` |
| `cpu_timing_test6/` | Shay Green (blargg) | Instruction cycle counts incl. page-crossing cases | Screen text only |
| `cpu_interrupts_v2/` | Shay Green (blargg) | IRQ/NMI polling, CLI latency, interrupts during BRK and DMA | `$6000` |
| `ppu_vbl_nmi/` | Shay Green (blargg) | VBL flag timing, NMI control and timing, even/odd frames | `$6000` |
| `ppu_open_bus/` | Shay Green (blargg) | PPU open-bus decay behaviour | `$6000` |
| `sprite_hit_tests_2005.10.05/` | Shay Green (blargg) | Sprite 0 hit flag behaviour and timing | Zero page `$F8` + screen |
| `sprite_overflow_tests/` | Shay Green (blargg) | Sprite overflow flag behaviour and timing | Zero page `$F8` + screen |
| `apu_test/` | Shay Green (blargg) | APU length counters, frame IRQ, jitter, DMC | `$6000` |
| `apu_reset/` | Shay Green (blargg) | APU state at power-on and after reset | `$6000` (requests reset with `0x81`) |
| `oam_read/` | Shay Green (blargg) | `$2004` OAM reads | `$6000` |
| `oam_stress/` | Shay Green (blargg) | OAM read/write stress across power/reset alignments | `$6000` |
| `mmc3_test_2/` | Shay Green (blargg) | MMC3 IRQ counter, A12 clocking, scanline timing | `$6000` |

## Reporting protocols

**`$6000` (newer blargg shells).** While the test runs `$6000` holds
`0x80`. `0x81` means "press reset after at least 100 ms". On completion it
holds `0x00` for pass or a failure code. `$6001-$6003` contain the
signature `DE B0 61` whenever the status byte is valid, and `$6004`
onward is a NUL-terminated result message that mirrors the screen.

**Zero page `$F8` (2005-era blargg shells).** The running test number is
kept in `$F8`; on completion the ROM prints `PASSED` or `FAILED: #N`,
beeps `N` times, and parks the CPU on a `jmp *` loop. `$F8 == 1` means
pass, `0` means internal error, anything else is the failing test number
listed in the suite's readme.

**Screen only (`cpu_timing_test6`).** Prints `PASSED` or `FAIL OP :$xx`
into nametable 0; the harness decodes the nametable as ASCII.

**nestest.** Run from PC `$C000` with `P = $24`, `SP = $FD` and `CYC = 7`
and compare each instruction's registers and cycle count to
`nestest.log`. `nestest.txt` (Kevin Horton) documents the `$02`/`$03`
failure codes.

## Combined ROMs

`instr_test-v5/all_instrs.nes`, `official_only.nes`,
`cpu_interrupts_v2/cpu_interrupts.nes`, `ppu_vbl_nmi/ppu_vbl_nmi.nes` and
`apu_test/apu_test.nes` bundle their `rom_singles/` on an MMC1 board and
run them in sequence. The harness has a test for each combined ROM as
well as one per single, because the singles are NROM and give a precise
failure location.
