//! Shared helpers for the headless test-ROM harness.
//!
//! See docs/testing/TEST_ROM_HARNESS.md for the conventions each ROM family
//! uses to report results and for the current pass/ignore table.

// Each integration test binary compiles this module separately, so helpers
// that only one binary uses would otherwise trip dead_code under
// `clippy --all-targets -D warnings`.
#![allow(dead_code)]

use nes_emu::cartridge::Cartridge;
use nes_emu::system::System;
use std::path::PathBuf;

/// Absolute path of a file under `test-roms/`.
pub fn rom_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-roms")
        .join(rel)
}

/// Load a ROM from `test-roms/` into a fresh, reset `System`.
pub fn load_rom(rel: &str) -> System {
    let path = rom_path(rel);
    let cart = Cartridge::load_from_file(&path)
        .unwrap_or_else(|e| panic!("failed to load {}: {}", path.display(), e));
    let mut system = System::new();
    system.load_cartridge(cart);
    system
}

/// Decode nametable 0 ($2000-$23BF) as text, one line per tile row.
///
/// blargg's test shells write ASCII codes straight into the nametable as
/// tile indices, so the on-screen console can be read back verbatim.
/// Non-printable tiles become spaces; trailing blank lines are dropped.
pub fn screen_text(system: &System) -> String {
    let vram = &system.ppu.vram;
    let mut lines: Vec<String> = Vec::new();
    for row in 0..30 {
        let mut line = String::with_capacity(32);
        for col in 0..32 {
            let tile = vram[0x2000 + row * 32 + col];
            let ch = if (0x20..0x7F).contains(&tile) {
                tile as char
            } else {
                ' '
            };
            line.push(ch);
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// True when the CPU is parked on a `JMP *` (blargg's `forever:`/`exit:`
/// loop) or a KIL/JAM opcode. Used to detect the end of the older test
/// ROMs that do not use the $6000 protocol.
pub fn is_halted(system: &System) -> bool {
    let pc = system.pc();
    let opcode = system.peek(pc);
    match opcode {
        0x4C => system.peek_word(pc.wrapping_add(1)) == pc,
        0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 => true,
        _ => false,
    }
}

/// Outcome of running a ROM that follows the blargg $6000 protocol.
#[derive(Debug)]
pub struct BlarggResult {
    /// Final value of $6000 (0x00 = pass, 0x80 = still running when the
    /// frame cap was hit, anything else = failure code).
    pub status: u8,
    /// NUL-terminated message text from $6004.
    pub message: String,
    /// Decoded nametable text at the end of the run.
    pub screen: String,
    /// Number of frames executed.
    pub frames: u32,
    /// Number of times the ROM asked for a reset ($6000 == 0x81).
    pub resets: u32,
    /// Whether $6000 was ever observed as 0x80 (test running).
    pub seen_running: bool,
}

const BLARGG_SIGNATURE: [u8; 3] = [0xDE, 0xB0, 0x61];

fn blargg_signature_present(system: &System) -> bool {
    (0..3).all(|i| system.peek(0x6001 + i) == BLARGG_SIGNATURE[i as usize])
}

fn blargg_message(system: &System) -> String {
    let mut msg = String::new();
    let mut addr: u16 = 0x6004;
    while addr < 0x8000 {
        let b = system.peek(addr);
        if b == 0 {
            break;
        }
        msg.push(if (0x20..0x7F).contains(&b) || b == b'\n' {
            b as char
        } else {
            '?'
        });
        addr += 1;
    }
    msg.trim().to_string()
}

/// Run a $6000-protocol ROM until it reports a result or `max_frames`
/// elapse.
///
/// Protocol (from blargg's readmes): $6000 is 0x80 while the test runs,
/// 0x81 when it wants the reset button pressed (after a short delay),
/// 0x00 on pass and any other value on failure; $6001-$6003 hold the
/// signature DE B0 61 whenever the status byte is valid; $6004 onward is a
/// NUL-terminated result message. The status byte is only trusted once the
/// signature has been seen, because fresh PRG RAM also reads as 0x00.
pub fn run_blargg(rel: &str, max_frames: u32) -> BlarggResult {
    let mut system = load_rom(rel);
    let mut frames = 0u32;
    let mut resets = 0u32;
    let mut seen_running = false;
    let mut status = 0x80u8;

    while frames < max_frames {
        system.run_frame();
        frames += 1;

        if !blargg_signature_present(&system) {
            continue;
        }
        status = system.peek(0x6000);
        match status {
            0x80 => seen_running = true,
            0x81 => {
                // The ROM wants a reset, delayed by at least ~100 ms.
                for _ in 0..10 {
                    system.run_frame();
                    frames += 1;
                }
                system.reset();
                resets += 1;
                // After reset the shell spends a couple of vblanks on PPU
                // warm-up before it overwrites the 0x81 marker; wait for
                // that so a single request is not counted as several.
                let mut warmup = 0;
                while system.peek(0x6000) == 0x81 && warmup < 30 {
                    system.run_frame();
                    frames += 1;
                    warmup += 1;
                }
                seen_running = true;
            }
            // Pass (0x00) or a failure code. The signature check above
            // is what makes a 0x00 trustworthy even if we never observed
            // the 0x80 "running" state (fast ROMs finish within a frame).
            _ => break,
        }
    }

    BlarggResult {
        status,
        message: blargg_message(&system),
        screen: screen_text(&system),
        frames,
        resets,
        seen_running,
    }
}

/// Assert that a $6000-protocol ROM passes.
pub fn assert_blargg_passes(rel: &str, max_frames: u32) {
    let result = run_blargg(rel, max_frames);
    assert!(
        result.status == 0x00,
        "{} failed: status=0x{:02X} after {} frames ({} resets)\n--- $6004 message ---\n{}\n--- screen ---\n{}",
        rel,
        result.status,
        result.frames,
        result.resets,
        result.message,
        result.screen
    );
}

/// Outcome of running one of blargg's 2005-era ROMs (sprite hit, sprite
/// overflow, cpu_timing_test6) that report through zero page $F8 and the
/// on-screen console instead of $6000.
#[derive(Debug)]
pub struct LegacyResult {
    /// Zero page $F8: 1 = passed, 0 = internal error, N = failure code N.
    pub result_code: u8,
    /// Decoded nametable text.
    pub screen: String,
    /// Whether the CPU reached the terminal `jmp *` loop.
    pub halted: bool,
    pub frames: u32,
}

/// Run a legacy ROM until the CPU halts in its exit loop or `max_frames`
/// elapse.
pub fn run_legacy(rel: &str, max_frames: u32) -> LegacyResult {
    let mut system = load_rom(rel);
    let mut frames = 0u32;
    let mut halted = false;
    while frames < max_frames {
        system.run_frame();
        frames += 1;
        if is_halted(&system) {
            halted = true;
            break;
        }
    }
    LegacyResult {
        result_code: system.peek(0x00F8),
        screen: screen_text(&system),
        halted,
        frames,
    }
}

/// Assert that a $F8-protocol ROM halted with result code 1 (passed).
pub fn assert_legacy_passes(rel: &str, max_frames: u32) {
    let result = run_legacy(rel, max_frames);
    assert!(
        result.halted && result.result_code == 1,
        "{} failed: halted={} result_code={} after {} frames\n--- screen ---\n{}",
        rel,
        result.halted,
        result.result_code,
        result.frames,
        result.screen
    );
}

/// Assert that a ROM which only reports on screen printed `needle`.
pub fn assert_screen_contains(rel: &str, max_frames: u32, needle: &str) {
    let result = run_legacy(rel, max_frames);
    assert!(
        result.screen.contains(needle),
        "{} failed: expected screen text to contain {:?} (halted={}, {} frames)\n--- screen ---\n{}",
        rel,
        needle,
        result.halted,
        result.frames,
        result.screen
    );
}
