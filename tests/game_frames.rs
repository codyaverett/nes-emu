//! Framebuffer fingerprints for the commercial ROMs in `roms/`.
//!
//! Test ROMs do not cover rendering output, so this is the regression signal
//! for real games across timing and PPU changes. It is `#[ignore]`d because
//! `roms/` is not part of the repository; run it before and after a change
//! and diff the two outputs:
//!
//! ```text
//! cargo test --release --test game_frames -- --ignored --nocapture
//! ```
//!
//! Each ROM is run for a fixed number of frames with Start tapped once so
//! title screens advance, and the 256x240 RGB buffer is hashed at several
//! checkpoints. A changed hash is not necessarily a regression; it says a
//! human should look at that game.

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;

const CHECKPOINTS: [usize; 4] = [30, 120, 240, 400];
const START_AT: usize = 150;

fn fnv(buf: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(buf);
    h.finish()
}

#[test]
#[ignore = "needs commercial ROMs in roms/; prints fingerprints for manual diffing"]
fn fingerprint_commercial_roms() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("roms");
    let mut roms: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "nes"))
            .collect(),
        Err(_) => {
            println!("no roms/ directory; nothing to fingerprint");
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
        let mut sys = System::new();
        sys.load_cartridge(cart);
        let mut line = format!("{name:44}");
        let last = *CHECKPOINTS.last().unwrap();
        for f in 0..=last {
            if f == START_AT {
                sys.controller1.press(ControllerButton::START);
            } else if f == START_AT + 10 {
                sys.controller1.release(ControllerButton::START);
            }
            sys.run_frame();
            if CHECKPOINTS.contains(&f) {
                let fb = sys.get_frame_buffer();
                let nonblack = fb.iter().filter(|&&b| b != 0).count() > 0;
                line.push_str(&format!(
                    " f{f}:{:016x}{}",
                    fnv(fb),
                    if nonblack { "" } else { "(black)" }
                ));
            }
        }
        println!("{line}");
    }
}
