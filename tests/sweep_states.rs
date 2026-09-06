//! Build the gameplay save states that `tests/game_sweep.rs` starts from
//! (issue #41, docs/testing/COMPATIBILITY_SWEEP.md).
//!
//! Each game has a scripted path from power-on to the point where the
//! player is in control (a level, the overworld, the first fight), tuned
//! by looking at the frames. At the end of the path every button is
//! released, a few idle frames run so no held input is baked into the
//! image, and `System::save_state` writes `roms/<stem>.sweep.state`.
//! The state is then verified: a fresh `System` loads the ROM and the
//! image, and the first frame it draws must equal the frame the recipe
//! run draws next.
//!
//! ```text
//! # all games
//! cargo test --release --test sweep_states -- --ignored --nocapture
//! # one game, with a frame every 50 frames along the way for tuning
//! SWEEP_TRACE=/tmp/trace cargo test --release --test sweep_states -- --ignored --nocapture zelda
//! ```
//!
//! `SWEEP_OUT=<dir>` writes the verification frame of each state as a
//! PPM (`<stem>.state.ppm`). `SWEEP_TRACE=<dir>` writes a PPM every
//! `SWEEP_TRACE_EVERY` (default 50) frames during the recipe. Convert
//! with the stdlib-only script in docs/testing/COMPATIBILITY_SWEEP.md.
//!
//! Ignored because `roms/` is not part of the repository. A missing ROM
//! is reported and skipped, not a failure. State files are build-bound
//! (the layout of a section can change without a version bump); rerun
//! this test after any change to the save/load code.

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::{Path, PathBuf};

const A: ControllerButton = ControllerButton::A;
const B: ControllerButton = ControllerButton::B;
const START: ControllerButton = ControllerButton::START;
const SELECT: ControllerButton = ControllerButton::SELECT;
const UP: ControllerButton = ControllerButton::UP;
const DOWN: ControllerButton = ControllerButton::DOWN;
const LEFT: ControllerButton = ControllerButton::LEFT;
const RIGHT: ControllerButton = ControllerButton::RIGHT;

/// Frames a tapped button stays down. Most games poll once per frame and
/// debounce on the edge, so anything from 2 up works; 8 matches the
/// menu script in `game_sweep.rs`.
const TAP: usize = 8;

/// Idle frames run after the last input, with everything released,
/// before the image is taken.
const SETTLE: usize = 4;

/// Frames run after the load before the verification compares the two
/// machines.
const VERIFY: usize = 60;

#[derive(Clone, Copy)]
enum Ev {
    Press(ControllerButton),
    Release(ControllerButton),
}

/// A timed input script ending at `save_at`.
struct Recipe {
    rom: &'static str,
    events: Vec<(usize, Ev)>,
    save_at: usize,
}

impl Recipe {
    fn new(rom: &'static str) -> Self {
        Recipe {
            rom,
            events: Vec::new(),
            save_at: 0,
        }
    }

    /// Press `b` at `frame` and release it `TAP` frames later.
    fn tap(mut self, frame: usize, b: ControllerButton) -> Self {
        self.events.push((frame, Ev::Press(b)));
        self.events.push((frame + TAP, Ev::Release(b)));
        self
    }

    /// Hold `b` from `from` up to (not including) `to`.
    fn hold(mut self, from: usize, to: usize, b: ControllerButton) -> Self {
        self.events.push((from, Ev::Press(b)));
        self.events.push((to, Ev::Release(b)));
        self
    }

    /// Tap `b` every `every` frames in `[from, to)`.
    fn mash(mut self, from: usize, to: usize, every: usize, b: ControllerButton) -> Self {
        let mut f = from;
        while f < to {
            self = self.tap(f, b);
            f += every;
        }
        self
    }

    fn save_at(mut self, frame: usize) -> Self {
        self.save_at = frame;
        self
    }
}

fn fnv(buf: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(buf);
    h.finish()
}

fn write_ppm(path: &Path, rgb: &[u8]) {
    let mut out = b"P6\n256 240\n255\n".to_vec();
    out.extend_from_slice(rgb);
    std::fs::write(path, out).unwrap();
}

fn roms_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("roms")
}

fn env_dir(var: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var(var).ok()?);
    std::fs::create_dir_all(&dir).unwrap();
    Some(dir)
}

fn load(rom: &Path) -> Option<System> {
    let bytes = match std::fs::read(rom) {
        Ok(b) => b,
        Err(e) => {
            println!("{}: not available ({e}); skipped", rom.display());
            return None;
        }
    };
    let cart = match Cartridge::load_from_bytes(&bytes) {
        Ok(c) => c,
        Err(e) => {
            println!("{}: load failed: {e}; skipped", rom.display());
            return None;
        }
    };
    let mut sys = System::new();
    sys.load_cartridge(cart);
    Some(sys)
}

/// Run the recipe, save the state next to the ROM, verify it in a fresh
/// machine. Returns the state path, or `None` when the ROM is missing.
fn build(recipe: Recipe) -> Option<PathBuf> {
    let rom = roms_dir().join(recipe.rom);
    let stem = rom.file_stem().unwrap().to_string_lossy().into_owned();
    let mut sys = load(&rom)?;
    let trace = env_dir("SWEEP_TRACE");
    let every: usize = std::env::var("SWEEP_TRACE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    for f in 0..=recipe.save_at {
        for (at, ev) in &recipe.events {
            if *at == f {
                match ev {
                    Ev::Press(b) => sys.controller1.press(*b),
                    Ev::Release(b) => sys.controller1.release(*b),
                }
            }
        }
        sys.run_frame();
        if let Some(dir) = &trace {
            if f % every == 0 {
                write_ppm(
                    &dir.join(format!("{stem}.{f:04}.ppm")),
                    sys.get_frame_buffer(),
                );
            }
        }
    }
    // Nothing held in the image: the INPT section stores the buttons and
    // a held Right would steer the sweep before its own script starts.
    for b in [A, B, START, SELECT, UP, DOWN, LEFT, RIGHT] {
        sys.controller1.release(b);
    }
    for _ in 0..SETTLE {
        sys.run_frame();
    }

    let image = sys.save_state();
    let path = rom.with_extension("sweep.state");
    std::fs::write(&path, &image).unwrap();

    // Verify in a fresh machine. The frame buffer is not in the image,
    // so the first frame is compared only as a hint (it differs when the
    // game has rendering off at the save point, mid-fade for example);
    // the assertion is that after VERIFY frames both machines draw the
    // same picture and are in the same state, byte for byte.
    let mut fresh = load(&rom)?;
    fresh
        .load_state(&std::fs::read(&path).unwrap())
        .expect("state written by save_state must load");
    sys.run_frame();
    fresh.run_frame();
    if fnv(sys.get_frame_buffer()) != fnv(fresh.get_frame_buffer()) {
        println!("{stem}: note: first frame after load differs (rendering off at the save point?)");
    }
    for _ in 1..VERIFY {
        sys.run_frame();
        fresh.run_frame();
    }
    assert_eq!(
        fnv(sys.get_frame_buffer()),
        fnv(fresh.get_frame_buffer()),
        "{stem}: frame {VERIFY} after load differs from the recipe run"
    );
    assert_eq!(
        sys.save_state(),
        fresh.save_state(),
        "{stem}: machine state diverged after load"
    );
    if let Some(dir) = env_dir("SWEEP_OUT") {
        write_ppm(
            &dir.join(format!("{stem}.state.ppm")),
            fresh.get_frame_buffer(),
        );
    }
    println!(
        "{stem}: saved {} ({} bytes) at frame {}, verified",
        path.display(),
        image.len(),
        recipe.save_at + SETTLE
    );
    Some(path)
}

// ---------------------------------------------------------------------
// Recipes. Frame numbers were tuned by viewing SWEEP_TRACE frames; the
// per-game notes are in docs/testing/COMPATIBILITY_SWEEP.md.
// ---------------------------------------------------------------------

/// Start on the title, then hold Right into World 1-1.
fn recipe_mario() -> Recipe {
    Recipe::new("mario.nes")
        .tap(150, START)
        .hold(200, 400, RIGHT)
        .save_at(400)
}

/// Start on the title and the story screen, A on the character select
/// (Mario), then the World 1-1 intro drop; hold Right once in control.
fn recipe_smb2() -> Recipe {
    Recipe::new("SuperMarioBros2.nes")
        .tap(150, START)
        .tap(300, START)
        .tap(450, A)
        .hold(700, 1000, RIGHT)
        .save_at(1000)
}

/// The documented map sequence: two Starts reach the map, the WORLD 1
/// panel closes by frame 550, hold Right 40 frames, A, Up, Right, A.
/// The first A on the level panel is ignored, the second enters 1-1.
fn recipe_smb3() -> Recipe {
    Recipe::new("SuperMarioBros3.nes")
        .tap(150, START)
        .tap(300, START)
        .hold(600, 640, RIGHT)
        .tap(700, A)
        .tap(760, UP)
        .tap(820, RIGHT)
        .tap(880, A)
        .save_at(1100)
}

/// Start on the title. With no saved files the select screen's cursor
/// starts on REGISTER YOUR NAME, so Start again opens registration; A
/// types one letter into file 1, Select three times moves the heart
/// past files 2 and 3 to REGISTER (the cycle skips END and wraps to
/// file 1), Start there returns to the select screen with the file
/// named and the cursor on it, and Start starts the game. Link is
/// standing on the overworld start screen by frame 820.
fn recipe_zelda() -> Recipe {
    Recipe::new("zelda.nes")
        .tap(150, START)
        .tap(300, START)
        .tap(400, A)
        .tap(460, SELECT)
        .tap(500, SELECT)
        .tap(540, SELECT)
        .tap(600, START)
        .tap(700, START)
        .save_at(1100)
}

/// The title finishes scrolling in by frame 250; Start, then the stage
/// 1 intro scroll, then hold Right.
fn recipe_contra() -> Recipe {
    Recipe::new("Contra.nes")
        .tap(300, START)
        .hold(600, 900, RIGHT)
        .save_at(900)
}

/// Start on the title (the copyright screen ignores it), the sewer intro
/// and the area 1 information screen play by themselves. The screen has
/// two pages of text that type out; Start after each page is complete
/// (about frames 800 and 1150) leaves it.
fn recipe_tmnt() -> Recipe {
    Recipe::new("Teenage Mutant Ninja Turtles (USA).nes")
        .tap(150, START)
        .tap(400, START)
        .tap(950, START)
        .tap(1250, START)
        .save_at(1500)
}

/// The menu script from `game_sweep.rs`: four Starts through the title
/// and party screen, then A taps name every character AAAA; the
/// overworld is up by frame 1550 and the party is walking by 2400.
fn recipe_final_fantasy() -> Recipe {
    Recipe::new("final_fantasy.nes")
        .tap(150, START)
        .tap(300, START)
        .tap(450, START)
        .tap(600, START)
        .mash(720, 2400, 40, A)
        .save_at(2400)
}

/// Start skips the Technos logo, Start again picks 1P PLAY with Alex,
/// Start accepts the message speed and skill level screen, then hold
/// Right into the first fight.
fn recipe_river_city_ransom() -> Recipe {
    Recipe::new("river_city_ransom.nes")
        .tap(150, START)
        .tap(300, START)
        .tap(500, START)
        .hold(700, 1100, RIGHT)
        .save_at(1100)
}

/// The legal screen ignores Start until about frame 250. Start (title),
/// Start (game type, A-Type), Start (level select) into a game.
fn recipe_tetris() -> Recipe {
    Recipe::new("Tetris.nes")
        .tap(300, START)
        .tap(450, START)
        .tap(600, START)
        .tap(750, START)
        .save_at(900)
}

/// Start launches the highlighted entry (Bomberman), Start on its title
/// begins stage 1. Any later Start would pause it.
fn recipe_multicart() -> Recipe {
    Recipe::new("1200-in-1.nes")
        .tap(150, START)
        .tap(400, START)
        .save_at(700)
}

macro_rules! state_test {
    ($name:ident, $recipe:ident) => {
        #[test]
        #[ignore = "needs commercial ROMs in roms/; writes roms/<stem>.sweep.state"]
        fn $name() {
            build($recipe());
        }
    };
}

state_test!(mario, recipe_mario);
state_test!(smb2, recipe_smb2);
state_test!(smb3, recipe_smb3);
state_test!(zelda, recipe_zelda);
state_test!(contra, recipe_contra);
state_test!(tmnt, recipe_tmnt);
state_test!(final_fantasy, recipe_final_fantasy);
state_test!(river_city_ransom, recipe_river_city_ransom);
state_test!(tetris, recipe_tetris);
state_test!(multicart, recipe_multicart);
