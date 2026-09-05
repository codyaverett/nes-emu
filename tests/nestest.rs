//! nestest golden-log comparison.
//!
//! nestest.nes (Kevin Horton) exercises every official and unofficial
//! 6502 opcode. Started from PC $C000 with P = 0x24 and SP = 0xFD it runs
//! without needing a PPU, and `nestest.log` is the Nintendulator trace of
//! that run. Each test below steps the emulator one instruction at a time,
//! renders a trace line via `System::trace_line`, and compares parsed
//! fields against the log, reporting the first mismatch.

mod common;

use common::load_rom;
use nes_emu::system::System;
use std::fs;

const NESTEST_ROM: &str = "nestest/nestest.nes";
const NESTEST_LOG: &str = "nestest/nestest.log";

/// Registers (and cycle count) parsed out of one trace line.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct TraceFields {
    pc: u16,
    a: u8,
    x: u8,
    y: u8,
    p: u8,
    sp: u8,
    cyc: u64,
    /// PPU position `(scanline, dot)` from the `PPU:` column.
    ppu: (u16, u16),
}

fn field_after<'a>(line: &'a str, tag: &str) -> &'a str {
    let start = line
        .find(tag)
        .unwrap_or_else(|| panic!("trace line missing {:?}: {}", tag, line))
        + tag.len();
    let rest = &line[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    &rest[..end]
}

fn parse_trace(line: &str) -> TraceFields {
    let hex8 = |tag: &str| u8::from_str_radix(field_after(line, tag), 16).unwrap();
    TraceFields {
        pc: u16::from_str_radix(&line[0..4], 16).unwrap(),
        a: hex8("A:"),
        x: hex8("X:"),
        y: hex8("Y:"),
        p: hex8("P:"),
        sp: hex8("SP:"),
        cyc: field_after(line, "CYC:").parse().unwrap(),
        ppu: parse_ppu(line),
    }
}

/// The `PPU:` column is `PPU:sss,ddd` with space padding inside the
/// numbers, so it cannot go through `field_after`.
fn parse_ppu(line: &str) -> (u16, u16) {
    let start = line.find("PPU:").expect("trace line missing PPU:") + 4;
    let end = line[start..].find("CYC:").expect("trace line missing CYC:") + start;
    let mut parts = line[start..end].split(',');
    let scanline = parts.next().unwrap().trim().parse().unwrap();
    let dot = parts.next().unwrap().trim().parse().unwrap();
    (scanline, dot)
}

fn load_expected_log() -> Vec<String> {
    let path = common::rom_path(NESTEST_LOG);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e))
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Put the system into nestest's documented "automation" start state.
fn nestest_system() -> System {
    let mut system = load_rom(NESTEST_ROM);
    system.set_pc(0xC000);
    system.set_reg_a(0);
    system.set_reg_x(0);
    system.set_reg_y(0);
    system.set_reg_p(0x24);
    system.set_reg_sp(0xFD);
    // The golden log starts at CYC:7, the cost of the reset sequence.
    system.set_total_cpu_cycles(7);
    system
}

/// Outcome of a full log comparison.
struct Comparison {
    /// (1-based line number, expected line, actual line) of the first
    /// register mismatch, if any.
    first_reg_mismatch: Option<(usize, String, String)>,
    /// Same for the first CYC-column mismatch.
    first_cyc_mismatch: Option<(usize, String, String)>,
    /// Same for the first PPU-column (scanline, dot) mismatch.
    first_ppu_mismatch: Option<(usize, String, String)>,
    lines_compared: usize,
}

/// Step through the whole log. Stops at the first register mismatch (after
/// which the machine state has diverged and further lines are meaningless)
/// but keeps going past cycle-only mismatches so both kinds are reported.
fn compare_against_log(max_lines: usize) -> Comparison {
    let expected = load_expected_log();
    let mut system = nestest_system();
    let mut cmp = Comparison {
        first_reg_mismatch: None,
        first_cyc_mismatch: None,
        first_ppu_mismatch: None,
        lines_compared: 0,
    };

    for (idx, exp_line) in expected.iter().take(max_lines).enumerate() {
        let actual_line = system.trace_line();
        let exp = parse_trace(exp_line);
        let act = parse_trace(&actual_line);
        cmp.lines_compared = idx + 1;

        let regs_match = exp.pc == act.pc
            && exp.a == act.a
            && exp.x == act.x
            && exp.y == act.y
            && exp.p == act.p
            && exp.sp == act.sp;
        if !regs_match {
            cmp.first_reg_mismatch = Some((idx + 1, exp_line.clone(), actual_line));
            break;
        }
        if exp.cyc != act.cyc && cmp.first_cyc_mismatch.is_none() {
            cmp.first_cyc_mismatch = Some((idx + 1, exp_line.clone(), actual_line.clone()));
        }
        if exp.ppu != act.ppu && cmp.first_ppu_mismatch.is_none() {
            cmp.first_ppu_mismatch = Some((idx + 1, exp_line.clone(), actual_line));
        }
        system.step_instruction();
    }
    cmp
}

fn describe(kind: &str, m: &(usize, String, String)) -> String {
    format!(
        "nestest {} mismatch at log line {}\n  expected: {}\n  actual:   {}",
        kind, m.0, m.1, m.2
    )
}

#[test]
fn nestest_trace_format_matches_log_layout() {
    // The first log line is deterministic and independent of CPU
    // behaviour, so it pins the trace format itself.
    let system = nestest_system();
    let line = system.trace_line();
    let expected = &load_expected_log()[0];
    assert_eq!(parse_trace(&line), parse_trace(expected));
    assert!(
        line.starts_with("C000  4C F5 C5  JMP $C5F5"),
        "unexpected trace prefix: {}",
        line
    );
}

/// The log is known to match up to this line on the current CPU core; the
/// prefix test guards against regressions while the full compare is
/// ignored.
const KNOWN_GOOD_PREFIX: usize = 3639;

#[test]
fn nestest_registers_match_log_prefix() {
    let cmp = compare_against_log(KNOWN_GOOD_PREFIX);
    if let Some(m) = &cmp.first_reg_mismatch {
        panic!("{}", describe("register", m));
    }
    assert_eq!(cmp.lines_compared, KNOWN_GOOD_PREFIX);
}

#[test]
fn nestest_ppu_position_matches_log() {
    // The PPU column proves the PPU advances inside each instruction at the
    // right cycle, not just that the CPU cycle total is right.
    let cmp = compare_against_log(usize::MAX);
    if let Some(m) = &cmp.first_ppu_mismatch {
        panic!("{}", describe("PPU position", m));
    }
    assert_eq!(cmp.lines_compared, load_expected_log().len());
}

#[test]
fn nestest_registers_match_log() {
    let cmp = compare_against_log(usize::MAX);
    if let Some(m) = &cmp.first_reg_mismatch {
        panic!("{}", describe("register", m));
    }
    assert_eq!(cmp.lines_compared, load_expected_log().len());
}

#[test]
fn nestest_cycles_match_log_prefix() {
    let cmp = compare_against_log(KNOWN_GOOD_PREFIX);
    if let Some(m) = &cmp.first_reg_mismatch {
        panic!("{}", describe("register", m));
    }
    if let Some(m) = &cmp.first_cyc_mismatch {
        panic!("{}", describe("CYC", m));
    }
}

#[test]
fn nestest_cycles_match_log() {
    let cmp = compare_against_log(usize::MAX);
    // Report the earliest divergence of either kind.
    match (&cmp.first_cyc_mismatch, &cmp.first_reg_mismatch) {
        (Some(c), Some(r)) if c.0 <= r.0 => panic!("{}", describe("CYC", c)),
        (Some(c), None) => panic!("{}", describe("CYC", c)),
        (_, Some(r)) => panic!("{}", describe("register", r)),
        (None, None) => {}
    }
}

#[test]
fn nestest_result_bytes_are_clear() {
    // nestest.txt: in automation mode the last failing test number for the
    // official opcode group is stored at $02 and for the unofficial group
    // at $03; both are 0 on success.
    let expected = load_expected_log();
    let mut system = nestest_system();
    for _ in 0..expected.len() - 1 {
        system.step_instruction();
    }
    // Only trust $02/$03 if we actually followed the golden path to its
    // final instruction; otherwise a zero could just mean the tests never
    // ran.
    let final_pc = parse_trace(expected.last().unwrap()).pc;
    assert_eq!(
        system.pc(),
        final_pc,
        "run did not end on the log's final instruction, result bytes not meaningful"
    );
    let official = system.peek(0x0002);
    let unofficial = system.peek(0x0003);
    assert_eq!(
        (official, unofficial),
        (0, 0),
        "nestest reported failures: official=0x{:02X} unofficial=0x{:02X} (codes in test-roms/nestest/nestest.txt)",
        official,
        unofficial
    );
}
