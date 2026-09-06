//! Scripted gameplay sweep over the commercial ROMs in `roms/`.
//!
//! `tests/game_frames.rs` hashes frames near the title screen. This test
//! drives each game further and writes frames as binary PPM (viewable
//! directly, or convert to PNG with the stdlib-only script in
//! docs/testing/COMPATIBILITY_SWEEP.md) at four checkpoints.
//!
//! Two modes, chosen per ROM and printed:
//!
//! - **state**: `roms/<stem>.sweep.state` exists (built by
//!   `tests/sweep_states.rs`) and loads. The game starts inside gameplay
//!   and a per-game input script (`scripts`) plays it: platformers hold
//!   Right and tap A, the fighter alternates A and B, Tetris nudges and
//!   rotates, the RPG just walks. Frame numbers count from the load.
//! - **menu**: no state, or it failed to load (a state is bound to the
//!   build that wrote it; a layout error means rebuild it). The original
//!   script runs from power-on: Start at frames 150, 300, 450 and 600,
//!   then hold Right and tap A every 40 frames. Games that need a menu
//!   path stall at that screen; that is a limit of the script, not a bug.
//!
//! ```text
//! SWEEP_OUT=/tmp/sweep cargo test --release --test game_sweep -- --ignored --nocapture
//! ```
//!
//! `SWEEP_ONLY=<substring>` restricts the run to ROMs whose file name
//! contains it, for tuning one game's script.
//!
//! Ignored because `roms/` is not part of the repository and the output
//! needs a human to look at it. Results and the per-game verdicts are in
//! docs/testing/COMPATIBILITY_SWEEP.md.

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::collections::HashMap;
use std::path::Path;

const CHECKPOINTS: [usize; 4] = [400, 900, 1500, 2400];
const START_TAPS: [usize; 4] = [150, 300, 450, 600];
const PLAY_FROM: usize = 700;

/// Frames of settling after a state load before the script starts.
const STATE_PLAY_FROM: usize = 30;

/// How a game is played once it is in gameplay.
#[derive(Clone, Copy, Debug)]
enum Script {
    /// Hold Right, hold A for a full jump every 40 frames (jump, bomb,
    /// sword).
    Platformer,
    /// Hold Right, tap A every 40 frames: a short hop. Every longer jump
    /// tried in SMB3 1-1 lands Mario on the Goomba or the piranha plant
    /// by frame 900; the hop keeps him alive (stalled at the first pipe)
    /// for the whole run, which exercises the level renderer more than
    /// the map does.
    Hopper,
    /// Hold Left for 26 frames: walks onto the manhole to the left of
    /// the TMNT area 1 start, which drops into the first sewer on a
    /// ladder; from frame 200 hold Down to climb off it, then from 320
    /// hold Right and jump like a platformer.
    Sewer,
    /// Hold Right, alternate A and B taps every 30 frames (punch, kick).
    Fighter,
    /// Tap Right every 60 frames and A every 45 to rotate; pieces drift
    /// right and stack instead of piling in one column.
    Tetris,
    /// Hold Right only: A would open the menu on an RPG overworld.
    Walker,
}

/// ROM stem to play script. A stem not listed plays as a platformer.
fn scripts() -> HashMap<&'static str, Script> {
    HashMap::from([
        ("mario", Script::Platformer),
        ("SuperMarioBros2", Script::Platformer),
        ("SuperMarioBros3", Script::Hopper),
        ("Contra", Script::Platformer),
        ("zelda", Script::Platformer),
        ("Teenage Mutant Ninja Turtles (USA)", Script::Sewer),
        ("river_city_ransom", Script::Fighter),
        ("Tetris", Script::Tetris),
        ("final_fantasy", Script::Walker),
        ("1200-in-1", Script::Platformer),
    ])
}

fn write_ppm(path: &Path, rgb: &[u8]) {
    let mut out = b"P6\n256 240\n255\n".to_vec();
    out.extend_from_slice(rgb);
    std::fs::write(path, out).unwrap();
}

/// Frames a tapped button stays down; enough for every game's debounce.
const TAP: usize = 6;

/// Frames a jump button stays down: a full-height jump in the Mario
/// games needs A held for most of the rise.
const JUMP: usize = 24;

/// Press `b` for `hold` frames every `every` frames from `from`, offset
/// by `phase`.
fn tap(
    sys: &mut System,
    f: usize,
    from: usize,
    every: usize,
    phase: usize,
    hold: usize,
    b: ControllerButton,
) {
    if f < from {
        return;
    }
    let t = (f - from) % every;
    if t == phase {
        sys.controller1.press(b);
    } else if t == phase + hold {
        sys.controller1.release(b);
    }
}

/// The original power-on script: Starts through the menus, then play.
fn menu_script(sys: &mut System, f: usize) {
    for s in START_TAPS {
        if f == s {
            sys.controller1.press(ControllerButton::START);
        }
        if f == s + 8 {
            sys.controller1.release(ControllerButton::START);
        }
    }
    if f == PLAY_FROM {
        sys.controller1.press(ControllerButton::RIGHT);
    }
    tap(sys, f, PLAY_FROM, 40, 0, TAP, ControllerButton::A);
}

/// Play from a loaded state. Never taps Start: it pauses most games.
fn play_script(sys: &mut System, f: usize, script: Script) {
    let from = STATE_PLAY_FROM;
    match script {
        Script::Platformer => {
            if f == from {
                sys.controller1.press(ControllerButton::RIGHT);
            }
            tap(sys, f, from, 40, 0, JUMP, ControllerButton::A);
        }
        Script::Hopper => {
            if f == from {
                sys.controller1.press(ControllerButton::RIGHT);
            }
            tap(sys, f, from, 40, 0, TAP, ControllerButton::A);
        }
        Script::Sewer => {
            if f == from {
                sys.controller1.press(ControllerButton::LEFT);
            }
            if f == from + 26 {
                sys.controller1.release(ControllerButton::LEFT);
            }
            if f == from + 200 {
                sys.controller1.press(ControllerButton::DOWN);
            }
            if f == from + 320 {
                sys.controller1.release(ControllerButton::DOWN);
                sys.controller1.press(ControllerButton::RIGHT);
            }
            tap(sys, f, from + 320, 40, 0, JUMP, ControllerButton::A);
        }
        Script::Fighter => {
            if f == from {
                sys.controller1.press(ControllerButton::RIGHT);
            }
            tap(sys, f, from, 60, 0, TAP, ControllerButton::A);
            tap(sys, f, from, 60, 30, TAP, ControllerButton::B);
        }
        Script::Tetris => {
            tap(sys, f, from, 60, 0, TAP, ControllerButton::RIGHT);
            tap(sys, f, from, 45, 10, TAP, ControllerButton::A);
        }
        Script::Walker => {
            if f == from {
                sys.controller1.press(ControllerButton::RIGHT);
            }
        }
    }
}

/// Load the ROM into a new machine; returns the mapper number too.
fn load(rom: &Path) -> Result<(System, u8), String> {
    let bytes = std::fs::read(rom).map_err(|e| e.to_string())?;
    let cart = Cartridge::load_from_bytes(&bytes).map_err(|e| e.to_string())?;
    let mapper = cart.mapper_id;
    let mut sys = System::new();
    sys.load_cartridge(cart);
    Ok((sys, mapper))
}

#[test]
#[ignore = "needs commercial ROMs in roms/ and a human to look at the frames"]
fn sweep_commercial_roms() {
    let out = match std::env::var("SWEEP_OUT") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            println!("set SWEEP_OUT to a directory to receive the frames");
            return;
        }
    };
    std::fs::create_dir_all(&out).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("roms");
    let mut roms: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "nes"))
            .collect(),
        Err(_) => {
            println!("no roms/ directory; nothing to sweep");
            return;
        }
    };
    roms.sort();
    // SWEEP_ONLY=<substring> restricts the run to matching file names.
    if let Ok(only) = std::env::var("SWEEP_ONLY") {
        roms.retain(|p| p.file_name().unwrap().to_string_lossy().contains(&only));
    }
    let scripts = scripts();

    for rom in roms {
        let name = rom.file_name().unwrap().to_string_lossy().into_owned();
        let stem = rom.file_stem().unwrap().to_string_lossy().into_owned();
        let (mut sys, mapper) = match load(&rom) {
            Ok(s) => s,
            Err(e) => {
                println!("{name}: load failed: {e}");
                continue;
            }
        };
        println!("{name}: mapper {mapper}");
        // State mode when the image exists and loads; on any error the
        // machine is rebuilt (a layout error can leave it half loaded)
        // and the menu script runs from power-on.
        let state_path = rom.with_extension("sweep.state");
        let mut script = None;
        if let Ok(image) = std::fs::read(&state_path) {
            match sys.load_state(&image) {
                Ok(()) => {
                    let s = scripts
                        .get(stem.as_str())
                        .copied()
                        .unwrap_or(Script::Platformer);
                    println!(
                        "{name}: mode state ({}), script {s:?}",
                        state_path.file_name().unwrap().to_string_lossy()
                    );
                    script = Some(s);
                }
                Err(e) => {
                    println!(
                        "{name}: state {} refused ({e}); rebuild it with tests/sweep_states.rs",
                        state_path.file_name().unwrap().to_string_lossy()
                    );
                    sys = load(&rom).unwrap().0;
                }
            }
        }
        if script.is_none() {
            println!("{name}: mode menu script");
        }

        let last = *CHECKPOINTS.last().unwrap();
        for f in 0..=last {
            match script {
                Some(s) => play_script(&mut sys, f, s),
                None => menu_script(&mut sys, f),
            }
            sys.run_frame();
            if CHECKPOINTS.contains(&f) {
                write_ppm(
                    &out.join(format!("{name}.{f:04}.ppm")),
                    sys.get_frame_buffer(),
                );
            }
        }
    }
}
