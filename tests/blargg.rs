//! blargg (and related) test-ROM suites, one #[test] per ROM.
//!
//! Three reporting conventions are in play; see tests/common/mod.rs and
//! docs/testing/TEST_ROM_HARNESS.md:
//!
//! * `$6000` protocol (instr_test-v5, cpu_interrupts_v2, ppu_vbl_nmi,
//!   ppu_open_bus, apu_test, apu_reset, oam_read, oam_stress, mmc3_test_2)
//! * zero page `$F8` result code plus on-screen text (2005-era sprite hit
//!   and sprite overflow ROMs)
//! * on-screen text only (cpu_timing_test6)
//!
//! Suites that fail on the current emulator are `#[ignore]`d with the
//! observed failure so `cargo test` stays green; later phases un-ignore
//! them as they are fixed. Run everything, including ignored suites, with
//! `cargo test --test blargg -- --include-ignored`.
//!
//! Ignore reasons follow the pattern "<what the ROM reported>; <phase that
//! is expected to fix it>". "hangs" means $6000 was still 0x80 (or the
//! legacy ROM never reached its exit loop) when the frame budget ran out.

mod common;

use common::{assert_blargg_passes, assert_legacy_passes, assert_screen_contains};

/// Frame budgets (NTSC frames, 60 per second of emulated time).
const SHORT: u32 = 600; // 10 s
const MEDIUM: u32 = 1800; // 30 s
const LONG: u32 = 3600; // 60 s

macro_rules! blargg_test {
    ($name:ident, $rel:literal, $frames:expr) => {
        #[test]
        fn $name() {
            assert_blargg_passes($rel, $frames);
        }
    };
    ($name:ident, $rel:literal, $frames:expr, ignore = $reason:literal) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            assert_blargg_passes($rel, $frames);
        }
    };
}

macro_rules! legacy_test {
    ($name:ident, $rel:literal, $frames:expr) => {
        #[test]
        fn $name() {
            assert_legacy_passes($rel, $frames);
        }
    };
    ($name:ident, $rel:literal, $frames:expr, ignore = $reason:literal) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            assert_legacy_passes($rel, $frames);
        }
    };
}

macro_rules! screen_test {
    ($name:ident, $rel:literal, $frames:expr, $needle:literal) => {
        #[test]
        fn $name() {
            assert_screen_contains($rel, $frames, $needle);
        }
    };
    ($name:ident, $rel:literal, $frames:expr, $needle:literal, ignore = $reason:literal) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            assert_screen_contains($rel, $frames, $needle);
        }
    };
}

// ---------------------------------------------------------------------
// instr_test-v5: CPU instruction behaviour (official + unofficial)
// ---------------------------------------------------------------------
blargg_test!(
    instr_v5_01_basics,
    "instr_test-v5/rom_singles/01-basics.nes",
    SHORT
);
blargg_test!(
    instr_v5_02_implied,
    "instr_test-v5/rom_singles/02-implied.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_03_immediate,
    "instr_test-v5/rom_singles/03-immediate.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_04_zero_page,
    "instr_test-v5/rom_singles/04-zero_page.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_05_zp_xy,
    "instr_test-v5/rom_singles/05-zp_xy.nes",
    MEDIUM,
    ignore = "reports B4 LDY z,X (opcode $B4 unimplemented in cpu_step); CPU fix"
);
blargg_test!(
    instr_v5_06_absolute,
    "instr_test-v5/rom_singles/06-absolute.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_07_abs_xy,
    "instr_test-v5/rom_singles/07-abs_xy.nes",
    MEDIUM,
    ignore = "reports 9C SYA abs,X and 9E SXA abs,Y (unofficial SHY/SHX wrong); CPU fix"
);
blargg_test!(
    instr_v5_08_ind_x,
    "instr_test-v5/rom_singles/08-ind_x.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_09_ind_y,
    "instr_test-v5/rom_singles/09-ind_y.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_10_branches,
    "instr_test-v5/rom_singles/10-branches.nes",
    SHORT
);
blargg_test!(
    instr_v5_11_stack,
    "instr_test-v5/rom_singles/11-stack.nes",
    MEDIUM
);
blargg_test!(
    instr_v5_12_jmp_jsr,
    "instr_test-v5/rom_singles/12-jmp_jsr.nes",
    SHORT
);
blargg_test!(
    instr_v5_13_rts,
    "instr_test-v5/rom_singles/13-rts.nes",
    SHORT
);
blargg_test!(
    instr_v5_14_rti,
    "instr_test-v5/rom_singles/14-rti.nes",
    SHORT
);
blargg_test!(
    instr_v5_15_brk,
    "instr_test-v5/rom_singles/15-brk.nes",
    SHORT
);
blargg_test!(
    instr_v5_16_special,
    "instr_test-v5/rom_singles/16-special.nes",
    SHORT
);
blargg_test!(
    instr_v5_official_only,
    "instr_test-v5/official_only.nes",
    LONG,
    ignore = "fails in test 5 of 16 (B4 LDY z,X); CPU fix"
);
blargg_test!(
    instr_v5_all_instrs,
    "instr_test-v5/all_instrs.nes",
    LONG,
    ignore = "fails in test 5 of 16 (B4 LDY z,X), then 07-abs_xy; CPU fix"
);

// ---------------------------------------------------------------------
// cpu_timing_test6: instruction cycle counts (screen-only reporting)
// ---------------------------------------------------------------------
screen_test!(
    cpu_timing_test6,
    "cpu_timing_test6/cpu_timing_test.nes",
    MEDIUM,
    "PASSED",
    ignore = "screen shows FAIL OP :$B4 / UNKNOWN ERROR (opcode $B4 unimplemented); CPU fix then Phase 4"
);

// ---------------------------------------------------------------------
// cpu_interrupts_v2: IRQ/NMI polling and latency
// ---------------------------------------------------------------------
blargg_test!(
    cpu_interrupts_1_cli_latency,
    "cpu_interrupts_v2/rom_singles/1-cli_latency.nes",
    SHORT,
    ignore = "#3 APU should generate IRQ when $4017 = $00 (no IRQ line); Phase 3"
);
blargg_test!(
    cpu_interrupts_2_nmi_and_brk,
    "cpu_interrupts_v2/rom_singles/2-nmi_and_brk.nes",
    SHORT,
    ignore = "hangs after printing NMI BRK header (no IRQ line); Phase 3"
);
blargg_test!(
    cpu_interrupts_3_nmi_and_irq,
    "cpu_interrupts_v2/rom_singles/3-nmi_and_irq.nes",
    SHORT,
    ignore = "hangs after printing NMI BRK header (no IRQ line); Phase 3"
);
blargg_test!(
    cpu_interrupts_4_irq_and_dma,
    "cpu_interrupts_v2/rom_singles/4-irq_and_dma.nes",
    SHORT,
    ignore = "fails, prints constant 53 for every DMA offset (no IRQ line, DMA not cycle-stepped); Phase 3/4"
);
blargg_test!(
    cpu_interrupts_5_branch_delays_irq,
    "cpu_interrupts_v2/rom_singles/5-branch_delays_irq.nes",
    SHORT,
    ignore = "hangs in test_jmp (no IRQ line); Phase 3"
);
blargg_test!(
    cpu_interrupts_all,
    "cpu_interrupts_v2/cpu_interrupts.nes",
    MEDIUM,
    ignore = "fails in test 1 of 5 (APU frame IRQ never generated); Phase 3"
);

// ---------------------------------------------------------------------
// ppu_vbl_nmi: vblank flag and NMI timing
// ---------------------------------------------------------------------
blargg_test!(
    ppu_vbl_nmi_01_vbl_basics,
    "ppu_vbl_nmi/rom_singles/01-vbl_basics.nes",
    SHORT,
    ignore = "#8 VBL period is too long with BG off; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_02_vbl_set_time,
    "ppu_vbl_nmi/rom_singles/02-vbl_set_time.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_03_vbl_clear_time,
    "ppu_vbl_nmi/rom_singles/03-vbl_clear_time.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_04_nmi_control,
    "ppu_vbl_nmi/rom_singles/04-nmi_control.nes",
    SHORT,
    ignore = "#11 Immediate occurence should be after NEXT instruction; Phase 3/4"
);
blargg_test!(
    ppu_vbl_nmi_05_nmi_timing,
    "ppu_vbl_nmi/rom_singles/05-nmi_timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_06_suppression,
    "ppu_vbl_nmi/rom_singles/06-suppression.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_07_nmi_on_timing,
    "ppu_vbl_nmi/rom_singles/07-nmi_on_timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_08_nmi_off_timing,
    "ppu_vbl_nmi/rom_singles/08-nmi_off_timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4"
);
blargg_test!(
    ppu_vbl_nmi_09_even_odd_frames,
    "ppu_vbl_nmi/rom_singles/09-even_odd_frames.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);
blargg_test!(
    ppu_vbl_nmi_10_even_odd_timing,
    "ppu_vbl_nmi/rom_singles/10-even_odd_timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);
blargg_test!(
    ppu_vbl_nmi_all,
    "ppu_vbl_nmi/ppu_vbl_nmi.nes",
    MEDIUM,
    ignore = "fails in test 1 of 10 (VBL period too long with BG off); Phase 4"
);

// ---------------------------------------------------------------------
// ppu_open_bus
// ---------------------------------------------------------------------
blargg_test!(
    ppu_open_bus,
    "ppu_open_bus/ppu_open_bus.nes",
    SHORT,
    ignore = "#2 Write to any PPU register should set decay value; Phase 5"
);

// ---------------------------------------------------------------------
// sprite_hit_tests_2005.10.05 (legacy $F8 protocol)
// ---------------------------------------------------------------------
legacy_test!(
    sprite_hit_01_basics,
    "sprite_hit_tests_2005.10.05/01.basics.nes",
    SHORT,
    ignore = "FAILED #7 All-transparent sprite should miss; Phase 5"
);
legacy_test!(
    sprite_hit_02_alignment,
    "sprite_hit_tests_2005.10.05/02.alignment.nes",
    SHORT,
    ignore = "FAILED #2 Basic sprite-background alignment is way off; Phase 5"
);
legacy_test!(
    sprite_hit_03_corners,
    "sprite_hit_tests_2005.10.05/03.corners.nes",
    SHORT,
    ignore = "FAILED #2 Lower-right pixel should hit; Phase 5"
);
legacy_test!(
    sprite_hit_04_flip,
    "sprite_hit_tests_2005.10.05/04.flip.nes",
    SHORT,
    ignore = "FAILED #2; Phase 5"
);
legacy_test!(
    sprite_hit_05_left_clip,
    "sprite_hit_tests_2005.10.05/05.left_clip.nes",
    SHORT,
    ignore = "FAILED #4; Phase 5"
);
legacy_test!(
    sprite_hit_06_right_edge,
    "sprite_hit_tests_2005.10.05/06.right_edge.nes",
    SHORT,
    ignore = "FAILED #3; Phase 5"
);
legacy_test!(
    sprite_hit_07_screen_bottom,
    "sprite_hit_tests_2005.10.05/07.screen_bottom.nes",
    SHORT,
    ignore = "FAILED #6; Phase 5"
);
legacy_test!(
    sprite_hit_08_double_height,
    "sprite_hit_tests_2005.10.05/08.double_height.nes",
    SHORT,
    ignore = "FAILED #3; Phase 5"
);
legacy_test!(
    sprite_hit_09_timing_basics,
    "sprite_hit_tests_2005.10.05/09.timing_basics.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);
legacy_test!(
    sprite_hit_10_timing_order,
    "sprite_hit_tests_2005.10.05/10.timing_order.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);
legacy_test!(
    sprite_hit_11_edge_timing,
    "sprite_hit_tests_2005.10.05/11.edge_timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);

// ---------------------------------------------------------------------
// sprite_overflow_tests (legacy $F8 protocol)
// ---------------------------------------------------------------------
legacy_test!(
    sprite_overflow_1_basics,
    "sprite_overflow_tests/1.Basics.nes",
    SHORT,
    ignore = "FAILED #7 Should work normally when $2001 = $08 (bg only); Phase 5"
);
legacy_test!(
    sprite_overflow_2_details,
    "sprite_overflow_tests/2.Details.nes",
    SHORT,
    ignore = "FAILED #9 Shouldn't be set when all scanlines have 7 or fewer sprites; Phase 5"
);
legacy_test!(
    sprite_overflow_3_timing,
    "sprite_overflow_tests/3.Timing.nes",
    SHORT,
    ignore = "hangs in PPU sync loop; Phase 4/5"
);
legacy_test!(
    sprite_overflow_4_obscure,
    "sprite_overflow_tests/4.Obscure.nes",
    SHORT,
    ignore = "FAILED #7; Phase 5"
);
legacy_test!(
    sprite_overflow_5_emulator,
    "sprite_overflow_tests/5.Emulator.nes",
    SHORT
);

// ---------------------------------------------------------------------
// apu_test: frame counter, length counters, IRQ flag, DMC
// ---------------------------------------------------------------------
blargg_test!(
    apu_test_1_len_ctr,
    "apu_test/rom_singles/1-len_ctr.nes",
    SHORT,
    ignore = "#4 Writing $80 to $4017 should clock length immediately; Phase 3/4"
);
blargg_test!(
    apu_test_2_len_table,
    "apu_test/rom_singles/2-len_table.nes",
    SHORT,
    ignore = "fails on channel 0 length table; Phase 3/4"
);
blargg_test!(
    apu_test_3_irq_flag,
    "apu_test/rom_singles/3-irq_flag.nes",
    SHORT,
    ignore = "#4 Flag should be set in $4017 mode $00; Phase 3"
);
blargg_test!(
    apu_test_4_jitter,
    "apu_test/rom_singles/4-jitter.nes",
    SHORT,
    ignore = "#4 Even jitter not handled properly; Phase 4"
);
blargg_test!(
    apu_test_5_len_timing,
    "apu_test/rom_singles/5-len_timing.nes",
    SHORT,
    ignore = "#3 First length of mode 0 is too late; Phase 4"
);
blargg_test!(
    apu_test_6_irq_flag_timing,
    "apu_test/rom_singles/6-irq_flag_timing.nes",
    SHORT,
    ignore = "#2 Flag first set too soon; Phase 4"
);
blargg_test!(
    apu_test_7_dmc_basics,
    "apu_test/rom_singles/7-dmc_basics.nes",
    SHORT,
    ignore = "#2 DMC isn't working well enough to test further; Phase 3/4"
);
blargg_test!(
    apu_test_8_dmc_rates,
    "apu_test/rom_singles/8-dmc_rates.nes",
    SHORT,
    ignore = "#2 Rate 0's period is too short; Phase 4"
);
blargg_test!(
    apu_test_all,
    "apu_test/apu_test.nes",
    MEDIUM,
    ignore = "fails in test 1 of 8 (length table, timing, or $4015); Phase 3/4"
);

// ---------------------------------------------------------------------
// apu_reset: APU state at power and after reset (uses the 0x81 reset
// request)
// ---------------------------------------------------------------------
blargg_test!(
    apu_reset_4015_cleared,
    "apu_reset/4015_cleared.nes",
    SHORT,
    ignore = "#2 At power, $4015 should be cleared; Phase 3"
);
blargg_test!(
    apu_reset_4017_timing,
    "apu_reset/4017_timing.nes",
    SHORT,
    ignore = "#3 Frame IRQ flag should be set sooner after power/reset; Phase 3/4"
);
blargg_test!(
    apu_reset_4017_written,
    "apu_reset/4017_written.nes",
    SHORT,
    ignore = "#2 At power, $4017 should be written with $00; Phase 3"
);
blargg_test!(
    apu_reset_irq_flag_cleared,
    "apu_reset/irq_flag_cleared.nes",
    SHORT
);
blargg_test!(
    apu_reset_len_ctrs_enabled,
    "apu_reset/len_ctrs_enabled.nes",
    SHORT,
    ignore = "#3 At reset, length counters should be enabled, triangle unaffected; Phase 3"
);
blargg_test!(
    apu_reset_works_immediately,
    "apu_reset/works_immediately.nes",
    SHORT,
    ignore = "#2 At power, writes should work immediately; Phase 3"
);

// ---------------------------------------------------------------------
// OAM
// ---------------------------------------------------------------------
blargg_test!(oam_read, "oam_read/oam_read.nes", SHORT);
blargg_test!(
    oam_stress,
    "oam_stress/oam_stress.nes",
    LONG,
    ignore = "fails, OAM readback pattern shows every 4th byte wrong; Phase 5"
);

// ---------------------------------------------------------------------
// mmc3_test_2: MMC3 IRQ counter and A12 clocking
// ---------------------------------------------------------------------
blargg_test!(
    mmc3_1_clocking,
    "mmc3_test_2/rom_singles/1-clocking.nes",
    SHORT,
    ignore = "#3 Should decrement when A12 is toggled via PPUADDR (MMC3 IRQ counter never clocked); Phase 3"
);
blargg_test!(
    mmc3_2_details,
    "mmc3_test_2/rom_singles/2-details.nes",
    SHORT,
    ignore = "#2 Counter isn't working when reloaded with 255; Phase 3"
);
blargg_test!(
    mmc3_3_a12_clocking,
    "mmc3_test_2/rom_singles/3-A12_clocking.nes",
    SHORT,
    ignore = "#4 Should be clocked when A12 changes to 1 via PPUADDR write; Phase 5"
);
blargg_test!(
    mmc3_4_scanline_timing,
    "mmc3_test_2/rom_singles/4-scanline_timing.nes",
    SHORT,
    ignore = "hangs waiting for MMC3 IRQ; Phase 3"
);
blargg_test!(
    mmc3_5_mmc3,
    "mmc3_test_2/rom_singles/5-MMC3.nes",
    SHORT,
    ignore = "#2 Should reload and set IRQ every clock when reload is 0; Phase 3"
);
blargg_test!(
    mmc3_6_mmc3_alt,
    "mmc3_test_2/rom_singles/6-MMC3_alt.nes",
    SHORT,
    ignore = "#2 IRQ shouldn't be set when reloading to 0 (MMC3 alt revision); Phase 3"
);
