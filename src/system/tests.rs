//! Interrupt-line tests for the CPU: NMI edge, IRQ level, BRK/IRQ B flag,
//! and the sampling point (docs/debugging/INTERRUPT_LINE.md, issue 10).
//!
//! Each test builds a System with a synthetic 32 KB NROM cartridge whose PRG
//! is filled with NOPs and whose vectors point at distinct, recognisable
//! addresses. `cpu_step` is driven directly so the tests are independent of
//! the PPU frame loop.
//!
//! Interrupt inputs are sampled during the penultimate cycle of each
//! instruction, so a source that is already asserted when a test starts
//! stepping is first seen during the next instruction and serviced after
//! it: every "arm, then step" sequence below runs one instruction before
//! the 7-cycle interrupt sequence.

use super::System;
use crate::cartridge::Cartridge;

const RESET_VECTOR: u16 = 0x8000;
const NMI_VECTOR: u16 = 0x9000;
const IRQ_VECTOR: u16 = 0xA000;

const NOP: u8 = 0xEA;
const BRK: u8 = 0x00;
const CLI: u8 = 0x58;
const SEI: u8 = 0x78;
const PLP: u8 = 0x28;
const RTI: u8 = 0x40;
const JMP_ABS: u8 = 0x4C;
const BCC: u8 = 0x90;

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

/// Number of `apu.step()` calls after the `$4017` write before the frame
/// IRQ flag rises. `tick` performs exactly one `apu.step()`, so stepping
/// the APU `n - k` times by hand makes the flag rise on tick `k` of the
/// next instruction.
fn apu_steps_to_frame_irq() -> u32 {
    let mut sys = system();
    sys.write_byte(0x4017, 0x00);
    let mut steps = 0u32;
    while !sys.apu.irq_pending() {
        sys.apu.step();
        steps += 1;
        assert!(steps < 40_000, "APU frame IRQ never asserted");
    }
    steps
}

/// Arm the APU so the frame IRQ flag rises on tick `tick` of the next
/// instruction, with I clear.
fn arm_apu_irq_for_tick(sys: &mut System, tick: u32) {
    let n = apu_steps_to_frame_irq();
    sys.write_byte(0x4017, 0x00);
    for _ in 0..(n - tick) {
        sys.apu.step();
    }
    assert!(!sys.apu.irq_pending(), "flag must not be up yet");
    sys.cpu_status &= !0x04;
}

/// Push a return address and status byte as an interrupt would, so RTI or
/// PLP can pull them.
fn push_frame(sys: &mut System, pc: u16, p: u8) {
    sys.push_word(pc);
    sys.push(p);
}

#[test]
fn irq_taken_when_i_clear_and_apu_frame_flag_set() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04; // clear I

    // The line was already high, so it is seen during the first tick of
    // this NOP (its penultimate cycle) and serviced after it.
    assert_eq!(sys.cpu_step(), 2, "one instruction runs first");
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
fn cli_takes_effect_one_instruction_late() {
    // The line is level-triggered and held, but CLI's poll still uses the
    // old I flag: the instruction after CLI runs before the IRQ is taken.
    let mut sys = system_with_rom(|prg| prg[0] = CLI);
    raise_apu_frame_irq(&mut sys);

    assert_eq!(sys.cpu_step(), 2, "CLI executes normally");
    assert_eq!(sys.cpu_status & 0x04, 0, "the flag itself changes at once");
    assert_eq!(
        sys.cpu_step(),
        2,
        "the next instruction runs before the IRQ"
    );
    assert_eq!(sys.cpu_pc, RESET_VECTOR + 2);
    assert_eq!(sys.cpu_step(), 7, "IRQ serviced after that");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn cli_sei_allows_exactly_one_irq_with_i_set_in_pushed_status() {
    // cpu_interrupts_v2 1-cli_latency tests 5 and 6.
    let mut sys = system_with_rom(|prg| {
        prg[0] = CLI;
        prg[1] = SEI;
    });
    raise_apu_frame_irq(&mut sys);

    assert_eq!(sys.cpu_step(), 2, "CLI");
    assert_eq!(sys.cpu_step(), 2, "SEI runs: CLI's poll used the old I");
    let sp_before = sys.cpu_sp;
    assert_eq!(sys.cpu_step(), 7, "SEI's poll used the old (clear) I");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_ne!(pushed_p & 0x04, 0, "the pushed status already has I set");
}

#[test]
fn plp_delays_i_flag_but_rti_does_not() {
    // PLP: the pulled I flag is polled one instruction late.
    let mut sys = system_with_rom(|prg| prg[0] = PLP);
    sys.mapper_irq = true;
    push_frame(&mut sys, 0, 0x20); // P with I clear (only the byte matters)
    assert_eq!(sys.cpu_step(), 4, "PLP");
    assert_eq!(sys.cpu_status & 0x04, 0);
    assert_eq!(sys.cpu_step(), 2, "one more instruction before the IRQ");
    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);

    // RTI: the pulled I flag is effective immediately.
    let mut sys = system_with_rom(|prg| prg[0] = RTI);
    sys.mapper_irq = true;
    push_frame(&mut sys, RESET_VECTOR + 0x10, 0x20);
    assert_eq!(sys.cpu_step(), 6, "RTI");
    assert_eq!(sys.cpu_pc, RESET_VECTOR + 0x10);
    assert_eq!(sys.cpu_step(), 7, "IRQ taken straight after RTI");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn irq_acknowledged_by_4015_read_stops_retriggering() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04;

    assert_eq!(sys.cpu_step(), 2);
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

    assert_eq!(sys.cpu_step(), 2);
    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn irq_asserted_on_penultimate_cycle_is_taken_after_that_instruction() {
    let mut sys = system();
    arm_apu_irq_for_tick(&mut sys, 1); // tick 1 of a 2-cycle NOP
    assert_eq!(sys.cpu_step(), 2, "NOP");
    assert!(sys.apu.irq_pending(), "flag rose inside the NOP");
    assert_eq!(sys.cpu_step(), 7, "IRQ taken after the NOP");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn irq_asserted_on_last_cycle_waits_one_more_instruction() {
    let mut sys = system();
    arm_apu_irq_for_tick(&mut sys, 2); // tick 2 of a 2-cycle NOP: its last
    assert_eq!(sys.cpu_step(), 2, "NOP");
    assert!(sys.apu.irq_pending(), "flag rose inside the NOP");
    assert_eq!(sys.cpu_step(), 2, "missed by the NOP's poll: next NOP runs");
    assert_eq!(sys.cpu_step(), 7, "IRQ taken after that");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn taken_branch_without_page_cross_delays_irq_from_its_second_cycle() {
    // JMP abs (3 cycles): an IRQ rising on tick 2 (penultimate) is taken
    // after the JMP.
    let mut sys = system_with_rom(|prg| {
        prg[0] = JMP_ABS;
        prg[1] = 0x03;
        prg[2] = 0x80;
    });
    arm_apu_irq_for_tick(&mut sys, 2);
    assert_eq!(sys.cpu_step(), 3, "JMP");
    assert_eq!(sys.cpu_step(), 7, "IRQ taken after the JMP");

    // BCC to the next instruction (taken, same page, 3 cycles): the same
    // tick-2 IRQ is only seen by the following instruction.
    let mut sys = system_with_rom(|prg| {
        prg[0] = BCC;
        prg[1] = 0x00;
    });
    assert_eq!(sys.cpu_status & 0x01, 0, "carry clear after reset");
    arm_apu_irq_for_tick(&mut sys, 2);
    assert_eq!(sys.cpu_step(), 3, "BCC taken");
    assert_eq!(sys.cpu_pc, RESET_VECTOR + 2);
    assert_eq!(sys.cpu_step(), 2, "the instruction after the branch runs");
    assert_eq!(sys.cpu_step(), 7, "IRQ taken after that");
    assert_eq!(sys.cpu_pc, IRQ_VECTOR);
}

#[test]
fn nmi_fires_once_per_edge() {
    let mut sys = system();

    // Simulate the PPU raising its NMI output edge.
    sys.ppu.nmi_interrupt = true;

    assert_eq!(sys.cpu_step(), 2, "the edge is seen during this NOP");
    assert!(!sys.ppu.nmi_interrupt, "the edge latch is consumed");
    let sp_before = sys.cpu_sp;
    assert_eq!(sys.cpu_step(), 7, "NMI sequence takes 7 cycles");
    assert_eq!(sys.cpu_pc, NMI_VECTOR, "NMI must vector through $FFFA");
    assert_eq!(sys.cpu_sp, sp_before.wrapping_sub(3));
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(
        pushed_p & 0x30,
        0x20,
        "NMI pushes P with B clear, bit 5 set"
    );

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
    assert_eq!(sys.cpu_step(), 2);
    assert_eq!(sys.cpu_step(), 7);
    assert_eq!(sys.cpu_pc, NMI_VECTOR);
}

#[test]
fn nmi_has_priority_over_irq() {
    let mut sys = system();
    raise_apu_frame_irq(&mut sys);
    sys.cpu_status &= !0x04;
    sys.ppu.nmi_interrupt = true;

    assert_eq!(sys.cpu_step(), 2);
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

#[test]
fn nmi_during_brk_hijacks_its_vector() {
    // cpu_interrupts_v2 2-nmi_and_brk: an NMI edge seen during the first
    // cycles of BRK sends it through $FFFA with B still set in the pushed
    // status, and the NMI is thereby serviced (no second NMI sequence).
    let mut sys = system_with_rom(|prg| prg[0] = BRK);
    sys.ppu.nmi_interrupt = true; // seen on BRK's first tick
    let sp_before = sys.cpu_sp;

    assert_eq!(sys.cpu_step(), 7, "BRK");
    assert_eq!(sys.cpu_pc, NMI_VECTOR, "hijacked to the NMI vector");
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(pushed_p & 0x30, 0x30, "still BRK's status byte");

    sys.cpu_status &= !0x04;
    let sp_after = sys.cpu_sp;
    for _ in 0..4 {
        assert_eq!(sys.cpu_step(), 2, "the NMI is not serviced twice");
    }
    assert_eq!(sys.cpu_sp, sp_after);
}

#[test]
fn nmi_during_irq_sequence_hijacks_its_vector() {
    let mut sys = system();
    sys.mapper_irq = true;
    sys.cpu_status &= !0x04;
    assert_eq!(sys.cpu_step(), 2);
    sys.ppu.nmi_interrupt = true; // seen on the IRQ sequence's first tick
    let sp_before = sys.cpu_sp;

    assert_eq!(sys.cpu_step(), 7, "IRQ sequence");
    assert_eq!(sys.cpu_pc, NMI_VECTOR, "hijacked to the NMI vector");
    let pushed_p = stack_byte(&sys, 0x0100 | sp_before.wrapping_sub(2) as u16);
    assert_eq!(pushed_p & 0x30, 0x20, "B clear as for any IRQ");
    assert_eq!(sys.cpu_step(), 2, "handler's first instruction runs");
}
