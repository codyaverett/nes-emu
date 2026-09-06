//! Headless proof that a Game Genie code changes game behaviour
//! (issue #31, docs/debugging/CHEAT_ENGINE.md).
//!
//! SXIOPO is the well-known Super Mario Bros infinite-lives code. It
//! decodes to $91D9:$AD, turning `DEC $075A` (CE 5A 07, the lives
//! decrement in the player-death routine) into `LDA $075A`. The test runs
//! the same input script twice, once without and once with the cheat, and
//! watches the lives counter at RAM $075A: it must drop in the control run
//! (proving a death was staged) and must not drop with the cheat.
//!
//! ```text
//! cargo test --release --test cheats_smb -- --ignored --nocapture
//! ```
//!
//! Ignored because `roms/` is not part of the repository.

use nes_emu::cartridge::Cartridge;
use nes_emu::cheat::Cheat;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::path::PathBuf;

const LIVES: u16 = 0x075A;
const PATCH_ADDR: u16 = 0x91D9;
const START_TAP: usize = 150;
const RUN_FROM: usize = 300;
const MAX_FRAMES: usize = 2400;

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("roms")
        .join("mario.nes")
}

fn load_smb() -> Option<System> {
    let data = std::fs::read(rom_path()).ok()?;
    let cart = Cartridge::load_from_bytes(&data).expect("mario.nes should parse");
    let mut sys = System::new();
    sys.load_cartridge(cart);
    Some(sys)
}

/// Tap Start on the title screen, then hold Right so Mario walks into the
/// first Goomba. Returns the lives counter after each frame from
/// `RUN_FROM` onward (SMB initialises $075A during the title screen, so
/// earlier values are not meaningful), stopping two seconds after the
/// first change so the post-death state is included.
fn run(sys: &mut System, label: &str) -> Vec<u8> {
    let mut trace = Vec::with_capacity(MAX_FRAMES);
    let mut changed_at = None;
    for f in 0..MAX_FRAMES {
        if f == START_TAP {
            sys.controller1.press(ControllerButton::START);
        }
        if f == START_TAP + 8 {
            sys.controller1.release(ControllerButton::START);
        }
        if f == RUN_FROM {
            sys.controller1.press(ControllerButton::RIGHT);
        }
        sys.run_frame();
        if f < RUN_FROM {
            continue;
        }
        let lives = sys.peek(LIVES);
        if let Some(prev) = trace.last().copied() {
            if lives != prev {
                println!("{label}: frame {f}: $075A {prev:02X} -> {lives:02X}");
                changed_at.get_or_insert(f);
            }
        }
        trace.push(lives);
        if changed_at.is_some_and(|c| f > c + 120) {
            break;
        }
    }
    println!(
        "{label}: ran {} frames, $075A at frame {RUN_FROM} {:02X}, final {:02X}",
        trace.len() + RUN_FROM,
        trace[0],
        trace[trace.len() - 1]
    );
    trace
}

#[test]
#[ignore = "needs roms/mario.nes (not in the repository)"]
fn sxiopo_keeps_smb_lives_from_decreasing() {
    let Some(mut control) = load_smb() else {
        println!("roms/mario.nes not found; skipping");
        return;
    };
    // The bytes the code patches: the opcode of DEC $075A.
    assert_eq!(control.peek(PATCH_ADDR), 0xCE, "expected DEC abs at $91D9");
    assert_eq!(control.peek(PATCH_ADDR + 1), 0x5A);
    assert_eq!(control.peek(PATCH_ADDR + 2), 0x07);

    let control_trace = run(&mut control, "control");

    let mut cheated = load_smb().unwrap();
    cheated.cheats_mut().add(Cheat::parse("SXIOPO").unwrap());
    assert_eq!(cheated.cheats().rom_override(PATCH_ADDR, 0xCE), Some(0xAD));
    let cheat_trace = run(&mut cheated, "sxiopo");

    // Lives are stored minus one (2 = three lives shown). Both runs start
    // from the same power-on state, so the counter must only ever drop in
    // the control run.
    let control_first = control_trace[0];
    let control_min = *control_trace.iter().min().unwrap();
    let cheat_first = cheat_trace[0];
    let cheat_min = *cheat_trace.iter().min().unwrap();
    println!("control: first {control_first:02X} min {control_min:02X}; sxiopo: first {cheat_first:02X} min {cheat_min:02X}");
    assert_eq!(cheat_first, control_first);
    assert!(
        control_min < control_first,
        "control run never lost a life ($075A stayed {control_first:02X}); death not staged within {MAX_FRAMES} frames"
    );
    assert!(
        cheat_min >= cheat_first,
        "lives dropped with SXIOPO active: {cheat_min:02X} < {cheat_first:02X}"
    );
}
