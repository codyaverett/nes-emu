//! Save-state round trips (issue #39, docs/debugging/SAVE_STATES.md).
//!
//! The non-ignored tests use the blargg ROMs committed under `test-roms/`,
//! chosen so the awkward state is live when the snapshot is taken: the
//! MMC3 IRQ counter and A12 filter, a DMC sample in flight with a DMA
//! request outstanding, and the PPU NMI/vblank latches. Each runs the ROM
//! for a while, saves, runs on and hashes the frame buffer at several
//! checkpoints, loads the state and runs the same span again: the hashes
//! must match exactly. A save immediately after a load must also
//! reproduce the original image byte for byte, which is the check that
//! catches a field written by `save` but not restored by `load`.
//!
//! The Super Mario Bros. round trip needs `roms/mario.nes`, which is not
//! part of the repository:
//!
//! ```text
//! cargo test --test save_states -- --ignored --nocapture
//! ```

mod common;

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::state::{self, StateError};
use nes_emu::system::System;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn fnv(buf: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(buf);
    h.finish()
}

/// Run `frames` frames with audio capture on (so the audio filter state
/// moves too), discarding the samples.
fn run(system: &mut System, frames: usize) {
    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    for _ in 0..frames {
        system.run_frame_with_audio(Some(&buffer));
        buffer.lock().unwrap().clear();
    }
}

/// Run `frames` frames and hash the frame buffer at each checkpoint.
fn run_hashing(system: &mut System, frames: usize, checkpoints: &[usize]) -> Vec<u64> {
    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    let mut hashes = Vec::new();
    for f in 1..=frames {
        system.run_frame_with_audio(Some(&buffer));
        buffer.lock().unwrap().clear();
        if checkpoints.contains(&f) {
            hashes.push(fnv(system.get_frame_buffer()));
        }
    }
    hashes
}

/// Save, run `after` frames hashing at `checkpoints`, load, run again;
/// the two hash lists must match and the re-save must equal the image.
fn round_trip(system: &mut System, after: usize, checkpoints: &[usize]) {
    let image = system.save_state();
    let first = run_hashing(system, after, checkpoints);
    let end_of_first = system.save_state();
    assert_ne!(end_of_first, image, "the machine should have moved on");
    system.load_state(&image).expect("load_state");
    let resaved = system.save_state();
    assert_eq!(resaved, image, "save after load must reproduce the image");
    let second = run_hashing(system, after, checkpoints);
    assert_eq!(first, second, "frames after load differ from the first run");
    // Stronger than the picture: the whole machine must be where the
    // first run left it, byte for byte.
    assert_eq!(system.save_state(), end_of_first, "machine state diverged");
}

#[test]
fn mmc3_irq_counter_round_trips() {
    let mut system = common::load_rom("mmc3_test_2/rom_singles/4-scanline_timing.nes");
    run(&mut system, 40);
    round_trip(&mut system, 60, &[5, 20, 60]);
}

#[test]
fn dmc_dma_in_flight_round_trips() {
    let mut system = common::load_rom("apu_test/rom_singles/7-dmc_basics.nes");
    run(&mut system, 20);
    round_trip(&mut system, 40, &[3, 10, 40]);
}

#[test]
fn sprdma_and_dmc_dma_round_trips() {
    let mut system = common::load_rom("sprdma_and_dmc_dma/sprdma_and_dmc_dma.nes");
    run(&mut system, 30);
    round_trip(&mut system, 30, &[2, 15, 30]);
}

#[test]
fn ppu_vbl_nmi_round_trips() {
    let mut system = common::load_rom("ppu_vbl_nmi/rom_singles/06-suppression.nes");
    run(&mut system, 30);
    round_trip(&mut system, 40, &[4, 12, 40]);
}

#[test]
fn save_load_save_is_byte_identical_after_a_busy_run() {
    let mut system = common::load_rom("apu_test/apu_test.nes");
    run(&mut system, 120);
    let image = system.save_state();
    run(&mut system, 7);
    assert_ne!(system.save_state(), image);
    system.load_state(&image).unwrap();
    assert_eq!(system.save_state(), image);
}

#[test]
fn state_for_another_rom_is_refused() {
    let mut a = common::load_rom("apu_test/rom_singles/1-len_ctr.nes");
    let mut b = common::load_rom("ppu_vbl_nmi/rom_singles/01-vbl_basics.nes");
    run(&mut a, 5);
    run(&mut b, 5);
    let image_a = a.save_state();
    let before = b.save_state();
    let err = b.load_state(&image_a).unwrap_err();
    assert!(matches!(err, StateError::RomMismatch { .. }), "{err}");
    assert_eq!(b.save_state(), before, "a refused load changes nothing");
}

#[test]
fn truncated_state_is_refused_without_touching_the_machine() {
    let mut system = common::load_rom("apu_test/rom_singles/1-len_ctr.nes");
    run(&mut system, 10);
    let image = system.save_state();
    run(&mut system, 3);
    let before = system.save_state();
    for cut in [0, 3, 9, 20, image.len() / 2, image.len() - 1] {
        let err = system.load_state(&image[..cut]).unwrap_err();
        assert!(
            matches!(err, StateError::Truncated | StateError::BadMagic),
            "cut {cut}: {err}"
        );
        assert_eq!(system.save_state(), before, "cut {cut}");
    }
}

#[test]
fn no_cartridge_is_refused() {
    let loaded = common::load_rom("apu_test/rom_singles/1-len_ctr.nes");
    let image = loaded.save_state();
    let mut empty = System::new();
    assert_eq!(empty.load_state(&image), Err(StateError::NoCartridge));
}

#[test]
fn unknown_section_is_skipped_and_missing_section_refused() {
    let mut system = common::load_rom("apu_test/rom_singles/1-len_ctr.nes");
    run(&mut system, 10);
    let image = system.save_state();
    let parsed = state::parse(&image).unwrap();
    assert_eq!(parsed.sections.len(), state::REQUIRED_TAGS.len());

    // Rebuild the image with a made-up section spliced in after the RAM
    // section and another appended at the end.
    let mut w = state::header(parsed.rom_crc32);
    for (tag, payload) in &parsed.sections {
        w.section(tag, |w| w.bytes(payload));
        if tag == state::TAG_RAM {
            w.section(b"ZZZZ", |w| w.bytes(&[1, 2, 3, 4, 5]));
        }
    }
    w.section(b"TAIL", |w| w.u64(0xDEAD_BEEF));
    let spliced = w.into_bytes();
    assert!(spliced.len() > image.len());

    run(&mut system, 3);
    system.load_state(&spliced).unwrap();
    assert_eq!(system.save_state(), image, "unknown sections are dropped");

    // Drop a required section: refused before anything is applied.
    let mut w = state::header(parsed.rom_crc32);
    for (tag, payload) in &parsed.sections {
        if tag != state::TAG_APU {
            w.section(tag, |w| w.bytes(payload));
        }
    }
    let missing = w.into_bytes();
    run(&mut system, 3);
    let before = system.save_state();
    assert_eq!(
        system.load_state(&missing),
        Err(StateError::MissingSection("APU".into()))
    );
    assert_eq!(system.save_state(), before);
}

#[test]
fn short_section_is_a_truncation_and_long_section_a_layout_error() {
    let mut system = common::load_rom("apu_test/rom_singles/1-len_ctr.nes");
    run(&mut system, 5);
    let image = system.save_state();
    let parsed = state::parse(&image).unwrap();

    type Edit<'a> = &'a dyn Fn(&[u8; 4], &[u8], &mut state::Writer);
    let rebuild = |edit: Edit| {
        let mut w = state::header(parsed.rom_crc32);
        for (tag, payload) in &parsed.sections {
            w.section(tag, |w| edit(tag, payload, w));
        }
        w.into_bytes()
    };
    let short = rebuild(&|tag, payload, w| {
        let n = if tag == state::TAG_INPUT {
            payload.len() - 1
        } else {
            payload.len()
        };
        w.bytes(&payload[..n]);
    });
    assert_eq!(system.load_state(&short), Err(StateError::Truncated));

    let long = rebuild(&|tag, payload, w| {
        w.bytes(payload);
        if tag == state::TAG_INPUT {
            w.u8(0);
        }
    });
    assert_eq!(
        system.load_state(&long),
        Err(StateError::TrailingBytes("INPT".into(), 1))
    );
}

fn mario_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("roms")
        .join("mario.nes")
}

/// The issue's acceptance test: 400 frames of Super Mario Bros. with
/// Start tapped so the game is running, save, 300 more frames hashed at
/// 100/200/300, load, the same 300 frames again, identical hashes.
#[test]
#[ignore = "needs roms/mario.nes, which is not in the repository"]
fn super_mario_bros_round_trip_is_bit_exact() {
    let data = std::fs::read(mario_path()).expect("roms/mario.nes");
    let cart = Cartridge::load_from_bytes(&data).expect("mario.nes should parse");
    let mut system = System::new();
    system.load_cartridge(cart);

    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    for f in 0..400 {
        if f == 150 {
            system.controller1.press(ControllerButton::START);
        } else if f == 160 {
            system.controller1.release(ControllerButton::START);
        }
        system.run_frame_with_audio(Some(&buffer));
        buffer.lock().unwrap().clear();
    }
    // Hold Right so the run after the save has Mario moving and the
    // checkpoints differ from each other.
    system.controller1.press(ControllerButton::RIGHT);

    let image = system.save_state();
    println!("state image: {} bytes", image.len());
    let first = run_hashing(&mut system, 300, &[100, 200, 300]);
    system.load_state(&image).expect("load_state");
    assert_eq!(system.save_state(), image, "re-save after load");
    let second = run_hashing(&mut system, 300, &[100, 200, 300]);
    for (i, (a, b)) in first.iter().zip(&second).enumerate() {
        println!("frame +{}: {:016x} {:016x}", (i + 1) * 100, a, b);
    }
    assert_eq!(first, second);
    assert_ne!(first[0], first[2], "the game should be moving");
}
