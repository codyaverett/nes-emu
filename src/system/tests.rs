//! Interrupt-line tests for the CPU: NMI edge, IRQ level, BRK/IRQ B flag.
//!
//! Each test builds a System with a synthetic 32 KB NROM cartridge whose PRG
//! is filled with NOPs and whose vectors point at distinct, recognisable
//! addresses. `cpu_step` is driven directly so the tests are independent of
//! the PPU frame loop.

use super::System;
use crate::cartridge::Cartridge;

const RESET_VECTOR: u16 = 0x8000;
const NMI_VECTOR: u16 = 0x9000;
const IRQ_VECTOR: u16 = 0xA000;

const NOP: u8 = 0xEA;
const BRK: u8 = 0x00;
const CLI: u8 = 0x58;

/// Build an iNES image: 16-byte header, 32 KB PRG (mapper 0), no CHR ROM
/// (the loader zero-fills 8 KB of CHR RAM).
fn synthetic_rom() -> Vec<u8> {
    let mut rom = Vec::with_capacity(16 + 0x8000);
    rom.extend_from_slice(b"NES\x1A");
    rom.push(2); // 2 x 16 KB PRG
    rom.push(0); // no CHR ROM
    rom.push(0); // flags 6: mapper 0, horizontal mirroring
    rom.push(0); // flags 7
    rom.extend_from_slice(&[0; 8]);

    let mut prg = vec![NOP; 0x8000];
    prg[0x7FFA] = NMI_VECTOR as u8;
    prg[0x7FFB] = (NMI_VECTOR >> 8) as u8;
    prg[0x7FFC] = RESET_VECTOR as u8;
    prg[0x7FFD] = (RESET_VECTOR >> 8) as u8;
    prg[0x7FFE] = IRQ_VECTOR as u8;
    prg[0x7FFF] = (IRQ_VECTOR >> 8) as u8;
    rom.extend_from_slice(&prg);
    rom
}

fn system_with_rom(patch: impl FnOnce(&mut [u8])) -> System {
    let mut rom = synthetic_rom();
    patch(&mut rom[16..]);
    let cart = Cartridge::load_from_bytes(&rom).expect("synthetic ROM must parse");
    let mut system = System::new();
    system.load_cartridge(cart);
    assert_eq!(system.cpu_pc, RESET_VECTOR, "reset vector not honoured");
    system
}

fn system() -> System {
    system_with_rom(|_| {})
}

/// Read the byte at the given stack-page address.
fn stack_byte(system: &System, addr: u16) -> u8 {
    system.cpu_ram[(addr & 0x7FF) as usize]
}

/// Drive the APU through the real register path until the frame interrupt
/// flag is raised.
fn raise_apu_frame_irq(system: &mut System) {
    system.write_byte(0x4017, 0x00); // 4-step mode, IRQ not inhibited
    let mut budget = 40_000u32;
    while !system.apu.irq_pending() {
        system.apu.step();
        budget -= 1;
        assert!(budget > 0, "APU frame IRQ never asserted");
    }
}

#[test]
fn irq_taken_when_i_clear_and_apu_frame_flag_set() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04; // clear I
    let sp_before = sys.cpu_sp;
    let pc_before = sys.cpu_pc;

    let cycles = sys.cpu_step();

    assert_eq!(cycles, 7, "interrupt sequence takes 7 cycles");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR, "IRQ must vector through $FFFE");
    assert_eq!(sys.cpu_sp, sp_before.wrapping_sub(3), "IRQ pushes PC and P");
    assert_ne!(sys.cpu_status & 0x04, 0, "IRQ sets the I flag");

    // Pushed PC is the address of the instruction that would have run next.
    let pc_hi = stack_byte(&sys, 0x0100 | sp_before as u16);
    let pc_lo = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(1) as u16);
    assert_eq!(((pc_hi as u16) << 8) | pc_lo as u16, pc_before);

    // Pushed P has B clear and bit 5 set.
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(
        pushed_p & 0x30,
        0x20,
        "IRQ pushes P with B clear, bit 5 set"
    );
}

#[test]
fn irq_not_taken_when_i_set() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    assert_ne!(sys.cpu_status & 0x04, 0, "reset leaves I set");
    let sp_before = sys.cpu_sp;

    for _ in 0..8 {
        let cycles = sys.cpu_step();
        assert_eq!(cycles, 2, "only NOPs should execute while I is set");
    }

    assert_eq!(sys.cpu_sp, sp_before, "nothing pushed while I is set");
    assert_eq!(sys.cpu_pc, RESET_VECTOR + 8);
    assert!(
        sys.apu.irq_pending(),
        "the level stays asserted until acknowledged"
    );
}

#[test]
fn irq_taken_after_cli_while_line_held() {
    // The line is level-triggered: a pending source is picked up as soon as
    // software clears I, even though it was ignored earlier.
    let mut sys = system_with_rom(|prg| prg[0] = CLI);
    raise_apu_frame_irq(&mut sys);

    assert_eq!(sys.cpu_step(), 2, "CLI executes normally");
    assert_eq!(sys.cpu_status & 0x04, 0);
    assert_eq!(sys.cpu_step(), 7, "IRQ serviced at the next boundary");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn irq_acknowledged_by_4015_read_stops_retriggering() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04;

    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);

    // Handler acknowledges by reading $4015, then clears I.
    let status = sys.read_byte(0x4015);
    assert_ne!(status & 0x40, 0, "$4015 reports the frame IRQ");
    assert!(!sys.apu.irq_pending(), "$4015 read clears the frame flag");
    sys.cpu_status &= !0x04;

    let sp_before = sys.cpu_sp;
    for _ in 0..4 {
        assert_eq!(sys.cpu_step(), 2, "no further IRQ once acknowledged");
    }
    assert_eq!(sys.cpu_sp, sp_before);
}

#[test]
fn mapper_irq_input_drives_the_line() {
    let mut sys = system();
    sys.cpu_status &= !0x04;
    sys.mapper_irq = true;

    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn nmi_fires_once_per_edge() {
    let mut sys = system();
    let sp_before = sys.cpu_sp;

    // Simulate the PPU raising its NMI output edge.
    sys.ppu.nmi_interrupt = true;

    assert_eq!(sys.cpu_step(), 7, "NMI sequence takes 7 cycles");
    assert_eq!(sys.cpu_pc, NMI_VECTOR, "NMI must vector through $FFFA");
    assert_eq!(sys.cpu_sp, sp_before.wrapping_sub(3));
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(
        pushed_p & 0x30,
        0x20,
        "NMI pushes P with B clear, bit 5 set"
    );
    assert!(!sys.ppu.nmi_interrupt, "the edge latch is consumed");

    // With no new edge the NMI must not be serviced again, even with I clear.
    sys.cpu_status &= !0x04;
    let sp_after_nmi = sys.cpu_sp;
    for _ in 0..8 {
        assert_eq!(sys.cpu_step(), 2, "only NOPs run without a new edge");
    }
    assert_eq!(sys.cpu_sp, sp_after_nmi, "NMI serviced exactly once");
    assert_eq!(sys.cpu_pc, NMI_VECTOR + 8);

    // A second edge is serviced again.
    sys.ppu.nmi_interrupt = true;
    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, NMI_VECTOR);
}

#[test]
fn nmi_has_priority_over_irq() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04;
    sys.ppu.nmi_interrupt = true;

    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, NMI_VECTOR, "NMI wins when both are pending");
    // NMI set I, so the IRQ waits; it is still asserted for later.
    assert!(sys.apu.irq_pending());
}

#[test]
fn brk_pushes_b_flag_and_uses_irq_vector() {
    let mut sys = system_with_rom(|prg| prg[0] = BRK);
    let sp_before = sys.cpu_sp;

    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, IRQ_VECTOR, "BRK shares the $FFFE vector");
    assert_eq!(sys.cpu_sp, sp_before.wrapping_sub(3));

    // BRK pushes PC + 2 (the byte after the padding byte).
    let pc_hi = stack_byte(&sys, 0x0100 | sp_before as u16);
    let pc_lo = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(1) as u16);
    assert_eq!(((pc_hi as u16) << 8) | pc_lo as u16, RESET_VECTOR + 2);

    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(pushed_p & 0x30, 0x30, "BRK pushes P with B and bit 5 set");
    assert_eq!(
        sys.cpu_status & 0x10,
        0,
        "B is never set in the live status register"
    );
    assert_ne!(sys.cpu_status & 0x04, 0, "BRK sets the I flag");
}
