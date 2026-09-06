//! Battery-backed PRG RAM persistence (issue #26,
//! docs/debugging/BATTERY_SAVES.md).
//!
//! Each test builds a synthetic 32 KB NROM image and drives the public
//! `System::save_battery` / `System::load_battery` API, the same code path
//! the SDL binary uses. Save files go under `std::env::temp_dir()` with a
//! per-process, per-test name and are removed afterwards.

use nes_emu::cartridge::Cartridge;
use nes_emu::system::System;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const PRG_RAM_START: u16 = 0x6000;
const PRG_RAM_SIZE: usize = 0x2000;

/// 16-byte header plus 32 KB of NOPs with a reset vector at $8000.
/// `battery` sets flags 6 bit 1.
fn synthetic_rom(battery: bool) -> Vec<u8> {
    let mut rom = Vec::with_capacity(16 + 0x8000);
    rom.extend_from_slice(b"NES\x1A");
    rom.push(2); // 2 x 16 KB PRG
    rom.push(0); // CHR RAM
    rom.push(if battery { 0x02 } else { 0x00 });
    rom.push(0);
    rom.extend_from_slice(&[0; 8]);
    let mut prg = vec![0xEA; 0x8000];
    prg[0x7FFC] = 0x00;
    prg[0x7FFD] = 0x80;
    rom.extend_from_slice(&prg);
    rom
}

fn system_with(battery: bool) -> System {
    let cart = Cartridge::load_from_bytes(&synthetic_rom(battery)).expect("synthetic ROM");
    assert_eq!(cart.battery_backed, battery);
    let mut system = System::new();
    system.load_cartridge(cart);
    system
}

fn temp_sav(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "nes-emu-battery-{}-{}-{}.sav",
        tag,
        std::process::id(),
        nanos
    ))
}

/// Deterministic, non-trivial fill so a partial or shifted restore fails.
fn pattern(i: usize) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 5)) as u8
}

fn fill_prg_ram(system: &mut System) {
    for i in 0..PRG_RAM_SIZE {
        system.poke(PRG_RAM_START + i as u16, pattern(i));
    }
}

#[test]
fn battery_ram_round_trips_between_systems() {
    let path = temp_sav("roundtrip");

    let mut first = system_with(true);
    fill_prg_ram(&mut first);
    assert!(first.save_battery(&path).unwrap(), "first save writes");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), PRG_RAM_SIZE as u64);

    let mut second = system_with(true);
    assert_eq!(second.peek(PRG_RAM_START), 0, "fresh PRG RAM is zero");
    assert!(second.load_battery(&path).unwrap(), "save is loaded");
    for i in 0..PRG_RAM_SIZE {
        assert_eq!(
            second.peek(PRG_RAM_START + i as u16),
            pattern(i),
            "byte {i} after load"
        );
    }

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn unchanged_ram_is_not_rewritten() {
    let path = temp_sav("dirty");

    let mut system = system_with(true);
    fill_prg_ram(&mut system);
    assert!(system.save_battery(&path).unwrap());
    assert!(
        !system.save_battery(&path).unwrap(),
        "second save with no changes is skipped"
    );

    system.poke(PRG_RAM_START + 0x100, !pattern(0x100));
    assert!(
        system.save_battery(&path).unwrap(),
        "a changed byte is written"
    );
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk[0x100], !pattern(0x100));

    // Loading marks the current contents as saved as well.
    let mut other = system_with(true);
    assert!(other.load_battery(&path).unwrap());
    assert!(!other.save_battery(&path).unwrap());

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn non_battery_rom_never_writes_or_reads() {
    let path = temp_sav("nobattery");

    let mut system = system_with(false);
    fill_prg_ram(&mut system);
    assert!(!system.save_battery(&path).unwrap());
    assert!(!path.exists(), "no file for a non-battery cartridge");

    // Even with a file present, a non-battery cartridge ignores it.
    std::fs::write(&path, vec![0x55; PRG_RAM_SIZE]).unwrap();
    let mut fresh = system_with(false);
    assert!(!fresh.load_battery(&path).unwrap());
    assert_eq!(fresh.peek(PRG_RAM_START), 0);

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn missing_file_and_size_mismatch_are_ignored() {
    let path = temp_sav("mismatch");

    let mut system = system_with(true);
    assert!(
        !system.load_battery(&path).unwrap(),
        "missing file is Ok(false)"
    );

    std::fs::write(&path, vec![0xAA; PRG_RAM_SIZE / 2]).unwrap();
    assert!(
        !system.load_battery(&path).unwrap(),
        "wrong size is Ok(false)"
    );
    for i in 0..PRG_RAM_SIZE {
        assert_eq!(system.peek(PRG_RAM_START + i as u16), 0, "RAM untouched");
    }

    // The mismatched file is replaced on the next save.
    fill_prg_ram(&mut system);
    assert!(system.save_battery(&path).unwrap());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), PRG_RAM_SIZE as u64);

    std::fs::remove_file(&path).unwrap();
}

// ----------------------------------------------------------------------
// Byte-level API used by frontends without a file system (issue #49,
// docs/plans/WASM_WEB.md).
// ----------------------------------------------------------------------

#[test]
fn battery_bytes_round_trip_and_track_dirtiness() {
    let mut first = system_with(true);
    assert!(first.battery_dirty(), "never-saved RAM counts as dirty");
    first.mark_battery_saved();
    assert!(!first.battery_dirty());
    fill_prg_ram(&mut first);
    assert!(first.battery_dirty(), "poking RAM makes it dirty");
    let bytes = first.battery_ram().expect("battery backed").to_vec();
    assert_eq!(bytes.len(), PRG_RAM_SIZE);
    first.mark_battery_saved();
    assert!(!first.battery_dirty(), "mark_battery_saved clears the flag");

    let mut second = system_with(true);
    assert!(second.set_battery_ram(&bytes), "matching size is accepted");
    assert!(!second.battery_dirty(), "set_battery_ram counts as saved");
    for i in 0..PRG_RAM_SIZE {
        assert_eq!(second.peek(PRG_RAM_START + i as u16), pattern(i));
    }
    assert!(!second.set_battery_ram(&bytes[..10]), "wrong size rejected");
    assert_eq!(
        second.peek(PRG_RAM_START + 10),
        pattern(10),
        "RAM untouched"
    );
}

#[test]
fn battery_bytes_absent_without_battery_flag() {
    let mut system = system_with(false);
    assert!(system.battery_ram().is_none());
    assert!(!system.set_battery_ram(&[0; PRG_RAM_SIZE]));
    assert!(!system.battery_dirty());
    system.mark_battery_saved();
}
