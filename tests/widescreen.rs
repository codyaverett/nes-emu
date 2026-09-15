//! Wide view against Super Mario Bros: the right strip drawn at frame f
//! must match the picture that scrolls into view d pixels later.
//!
//! Ignored because `roms/` is not part of the repository:
//!
//! ```text
//! cargo test --release --test widescreen -- --ignored --nocapture
//! ```

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH, WIDE_EXT, WIDE_WIDTH};
use nes_emu::system::System;
use std::path::Path;

const FRAMES: usize = 2500;
/// SMB rows 0-31 hold the status bar (own scroll); skip them and the
/// bottom row.
const ROW_LO: usize = 32;
const ROW_HI: usize = 232;
const MAX_LOOKAHEAD: usize = 59;

#[test]
#[ignore = "needs roms/SuperMarioBros.nes; measures strip accuracy against later frames"]
fn smb_right_strip_matches_what_scrolls_in_later() {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR")).join("roms/SuperMarioBros.nes");
    let Ok(bytes) = std::fs::read(&rom) else {
        println!("no {}; skipping", rom.display());
        return;
    };
    let mut sys = System::new();
    sys.load_cartridge(Cartridge::load_from_bytes(&bytes).unwrap());
    sys.set_wide_enabled(true);

    // (world x, centre picture, wide frame)
    let mut samples: Vec<(usize, Vec<u8>, Vec<u8>)> = Vec::new();
    for f in 0..FRAMES {
        if f == 150 {
            sys.controller1.press(ControllerButton::START);
        }
        if f == 160 {
            sys.controller1.release(ControllerButton::START);
        }
        if f > 200 {
            sys.controller1.press(ControllerButton::RIGHT);
            if (f / 25) % 2 == 0 {
                sys.controller1.press(ControllerButton::A);
            } else {
                sys.controller1.release(ControllerButton::A);
            }
            sys.poke(0x079F, 0x10); // star timer: survive enemies
        }
        sys.run_frame();
        let world = sys.peek(0x071A) as usize * 256 + sys.peek(0x071C) as usize;
        samples.push((
            world,
            sys.get_frame_buffer().to_vec(),
            sys.get_wide_frame_buffer().to_vec(),
        ));
    }

    let mut equal = 0usize;
    let mut total = 0usize;
    let mut pairs = 0usize;
    for i in 0..samples.len() {
        let (wx, _, wide) = &samples[i];
        // First later frame whose scroll moved 8..=MAX_LOOKAHEAD px.
        let Some(j) = (i + 1..samples.len().min(i + 120))
            .find(|&j| samples[j].0 >= wx + 8 && samples[j].0 <= wx + MAX_LOOKAHEAD)
        else {
            continue;
        };
        let d = samples[j].0 - wx;
        if d > WIDE_EXT {
            continue;
        }
        pairs += 1;
        let centre_later = &samples[j].1;
        for y in ROW_LO..ROW_HI {
            for k in 0..d {
                // Strip pixel at x = 256 + k in frame i shows world column
                // wx + 256 + k, which frame j shows at picture x = 256 + k - d.
                let wi = (y * WIDE_WIDTH + WIDE_EXT + SCREEN_WIDTH + k) * 3;
                let ci = (y * SCREEN_WIDTH + SCREEN_WIDTH + k - d) * 3;
                total += 1;
                if wide[wi..wi + 3] == centre_later[ci..ci + 3] {
                    equal += 1;
                }
            }
        }
    }
    let pct = 100.0 * equal as f64 / total.max(1) as f64;
    println!("{pairs} frame pairs, {total} pixels compared, {pct:.1}% equal");
    assert!(pairs > 200, "not enough scrolling frames: {pairs}");
    // Sprites in the later picture are the main source of mismatch.
    assert!(pct > 90.0, "right strip accuracy {pct:.1}% below 90%");
    let _ = SCREEN_HEIGHT;
}
