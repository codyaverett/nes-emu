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

use super::{DmcDma, System};
use crate::cartridge::Cartridge;
use crate::input::ControllerButton;

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

// ---- DMC DMA (docs/debugging/DMC_DMA.md, issue 27) ----

/// Program the DMC for a non-looping sample at $C000 with IRQ disabled at
/// the fastest rate; `length` is the `$4013` value ((v << 4) | 1 bytes).
fn program_dmc(system: &mut System, length: u8) {
    system.write_byte(0x4010, 0x0F);
    system.write_byte(0x4012, 0x00);
    system.write_byte(0x4013, length);
}

#[test]
fn dmc_dma_halting_a_4016_read_clocks_the_controller_twice() {
    // LDA $4016 at the reset vector: opcode, two operand bytes, then the
    // controller read on the instruction's fourth cycle.
    let mut system = system_with_rom(|prg| {
        prg[0] = 0xAD;
        prg[1] = 0x16;
        prg[2] = 0x40;
    });
    system.controller1.press(ControllerButton::B);
    system.write_byte(0x4016, 0x01);
    system.write_byte(0x4016, 0x00);
    program_dmc(&mut system, 0x00);
    // The load DMA halts on the get cycle 3 or 4 cycles after the $4015
    // write (cycle W). For the halt to land on the $4016 read at W+4 the
    // cycle W+3 must be a put; shift the write by one cycle otherwise.
    if System::is_get_cycle(system.total_cycles + 1 + 3) {
        system.tick();
    }
    let write_cycle = system.total_cycles + 1;
    system.write_byte(0x4015, 0x10);
    assert_eq!(
        system.dmc_dma.map(|d| d.attempt),
        Some(write_cycle + 4),
        "load DMA scheduled on the get cycle 4 cycles after the write"
    );

    let cycles = system.cpu_step();
    assert_eq!(
        cycles,
        4 + 3,
        "load DMA stalls a read by 3 cycles (halt, dummy, get)"
    );
    // A single read would have returned bit 0 of A (released). The halt
    // cycle clocked the controller once and the repeated read once more,
    // so the CPU sees the second bit, B.
    assert_eq!(system.cpu_a & 1, 1, "CPU read the second shifted bit (B)");
    assert_eq!(
        system.read_byte(0x4016) & 1,
        0,
        "the next read is the third bit (Select)"
    );
    assert!(system.dmc_dma.is_none(), "request consumed by the fetch");
}

#[test]
fn reload_dma_stalls_four_cycles_and_can_halt_on_a_padded_cycle() {
    // A stream of NOPs: two cycles each, the second one padded (a dummy
    // read of the next opcode on hardware). 17-byte sample so several
    // reload DMAs follow the load.
    let mut system = system();
    program_dmc(&mut system, 0x01);
    system.write_byte(0x4015, 0x10);

    let mut stalls = Vec::new();
    let mut halt_ticks = Vec::new();
    let mut budget = 2000;
    while stalls.len() < 4 && budget > 0 {
        let cycles = system.cpu_step();
        if cycles != 2 {
            stalls.push(cycles - 2);
            halt_ticks.push(system.dmc_stall.0);
        }
        budget -= 1;
    }
    // The first fetch is the load DMA (3 cycles); every later one is a
    // reload DMA that halts on a put cycle and needs an alignment cycle:
    // 4 cycles, whether it lands on the opcode fetch or the padded cycle.
    assert_eq!(stalls, vec![3, 4, 4, 4]);
    // 432-cycle sample periods plus 4-cycle stalls keep the reloads in
    // phase with the 2-cycle NOPs, so every one lands on the same tick.
    assert_eq!(
        halt_ticks[1..],
        [2, 2, 2],
        "reloads halted on the NOP's padded second cycle"
    );
}

#[test]
fn dmc_dma_inside_oam_dma_costs_two_extra_cycles() {
    // STA $4014 at the reset vector, followed by NOPs.
    let mut system = system_with_rom(|prg| {
        prg[0] = 0x8D;
        prg[1] = 0x14;
        prg[2] = 0x40;
    });
    // Reference: OAM DMA alone, from the same cycle parity (the three
    // DMC register writes below tick too).
    let reference = {
        let mut sys = system_with_rom(|prg| {
            prg[0] = 0x8D;
            prg[1] = 0x14;
            prg[2] = 0x40;
        });
        program_dmc(&mut sys, 0x00);
        sys.cpu_step()
    };
    assert!(reference == 4 + 513 || reference == 4 + 514);

    // Schedule a reload-style request to land in the middle of the
    // transfer: the attempt tick is a put cycle well inside the DMA.
    program_dmc(&mut system, 0x00);
    system.apu.write_register(0x4015, 0x10);
    let attempt = System::next_cycle_with_parity(system.total_cycles + 200, false);
    system.dmc_dma = Some(DmcDma {
        attempt,
        halted_at: None,
    });
    let cycles = system.cpu_step();
    assert_eq!(
        cycles,
        reference + 2,
        "DMC get plus one OAM alignment cycle"
    );
    assert!(
        system.dmc_dma.is_none(),
        "sample byte fetched during OAM DMA"
    );
}

// ----------------------------------------------------------------------
// Cheat engine hooks (docs/debugging/CHEAT_ENGINE.md, issue #31).
// ----------------------------------------------------------------------

use crate::cheat::Cheat;

const LDA_IMM: u8 = 0xA9;

/// `LDA #$11` at the reset vector; the operand at $8001 is what the cheats
/// below target.
fn lda_system() -> System {
    system_with_rom(|prg| {
        prg[0] = LDA_IMM;
        prg[1] = 0x11;
    })
}

#[test]
fn rom_cheat_overrides_cpu_read() {
    let mut sys = lda_system();
    sys.cheats_mut().add(Cheat::parse("8001:22").unwrap());
    assert_eq!(sys.cpu_step(), 2);
    assert_eq!(sys.reg_a(), 0x22, "operand fetch went through the cheat");
    assert_eq!(sys.peek(0x8001), 0x11, "peek still shows the real ROM byte");
}

#[test]
fn rom_cheat_compare_mismatch_leaves_rom_alone() {
    let mut sys = lda_system();
    sys.cheats_mut().add(Cheat::parse("8001?99:22").unwrap());
    sys.cpu_step();
    assert_eq!(sys.reg_a(), 0x11);

    let mut sys = lda_system();
    sys.cheats_mut().add(Cheat::parse("8001?11:22").unwrap());
    sys.cpu_step();
    assert_eq!(sys.reg_a(), 0x22, "compare matches the real byte");
}

#[test]
fn disabled_rom_cheat_does_nothing() {
    let mut sys = lda_system();
    let idx = sys.cheats_mut().add(Cheat::parse("8001:22").unwrap());
    sys.cheats_mut().toggle(idx);
    assert!(!sys.cheats().is_active());
    sys.cpu_step();
    assert_eq!(sys.reg_a(), 0x11);
}

#[test]
fn ram_freeze_reapplied_each_frame() {
    let mut sys = system();
    sys.poke(0x0010, 0x05);
    sys.cheats_mut().add(Cheat::parse("0010:42").unwrap());
    sys.run_frame();
    assert_eq!(sys.peek(0x0010), 0x42, "frozen at frame start");
    // The game overwrites it mid-frame...
    sys.poke(0x0010, 0x07);
    assert_eq!(sys.peek(0x0010), 0x07);
    // ...and the next frame puts it back.
    sys.run_frame();
    assert_eq!(sys.peek(0x0010), 0x42);

    // Disabled: the game's value survives.
    sys.cheats_mut().set_enabled(0, false);
    sys.poke(0x0010, 0x07);
    sys.run_frame();
    assert_eq!(sys.peek(0x0010), 0x07);
}

#[test]
fn cheat_file_round_trip_through_system() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "nes-emu-cheats-{}-{}.cht",
        std::process::id(),
        nanos
    ));

    let mut sys = system();
    assert!(
        !sys.load_cheats(&path).unwrap(),
        "missing file is Ok(false)"
    );
    assert!(sys.cheats().is_empty());

    sys.cheats_mut().add(
        Cheat::parse("SXIOPO")
            .unwrap()
            .with_description("Infinite lives"),
    );
    sys.cheats_mut().add(Cheat::parse("0010:42").unwrap());
    sys.cheats_mut().set_enabled(1, false);
    assert!(sys.save_cheats(&path).unwrap());

    let mut other = system();
    assert!(other.load_cheats(&path).unwrap());
    assert_eq!(other.cheats(), sys.cheats());
    assert!(other.cheats().is_active());

    // A malformed file is InvalidData and leaves the set untouched.
    std::fs::write(&path, "NOTACODE\t1\tbad\n").unwrap();
    let err = other.load_cheats(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(other.cheats().len(), 2);

    let _ = std::fs::remove_file(&path);
}
