//! Scripted gameplay sweep over the commercial ROMs in `roms/`.
//!
//! `tests/game_frames.rs` hashes frames near the title screen. This test
//! drives each game further: Start is tapped four times to get through
//! title and menu screens, then Right is held and A tapped every 40 frames
//! so side-scrollers and fighters actually play. Frames are written as
//! binary PPM (viewable directly, or convert to PNG with the stdlib-only
//! script in docs/testing/COMPATIBILITY_SWEEP.md) at four checkpoints.
//!
//! ```text
//! SWEEP_OUT=/tmp/sweep cargo test --release --test game_sweep -- --ignored --nocapture
//! ```
//!
//! Ignored because `roms/` is not part of the repository and the output
//! needs a human to look at it. Results and the per-game verdicts are in
//! docs/testing/COMPATIBILITY_SWEEP.md.

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::path::Path;

const CHECKPOINTS: [usize; 4] = [400, 900, 1500, 2400];
const START_TAPS: [usize; 4] = [150, 300, 450, 600];
const PLAY_FROM: usize = 700;

fn write_ppm(path: &Path, rgb: &[u8]) {
    let mut out = b"P6\n256 240\n255\n".to_vec();
    out.extend_from_slice(rgb);
    std::fs::write(path, out).unwrap();
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

    for rom in roms {
        let name = rom.file_name().unwrap().to_string_lossy().into_owned();
        let cart = match Cartridge::load_from_bytes(&std::fs::read(&rom).unwrap()) {
            Ok(c) => c,
            Err(e) => {
                println!("{name}: load failed: {e}");
                continue;
            }
        };
        println!("{name}: mapper {}", cart.mapper_id);
        let mut sys = System::new();
        sys.load_cartridge(cart);
        let last = *CHECKPOINTS.last().unwrap();
        for f in 0..=last {
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
            if f >= PLAY_FROM && f % 40 == 0 {
                sys.controller1.press(ControllerButton::A);
            }
            if f >= PLAY_FROM && f % 40 == 6 {
                sys.controller1.release(ControllerButton::A);
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
