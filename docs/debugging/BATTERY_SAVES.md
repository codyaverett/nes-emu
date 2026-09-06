# Battery-Backed PRG RAM Saves

**Date:** 2026-09-05
**Type:** Feature / Persistence
**Severity:** High (every battery game lost all progress on quit)
**Status:** Complete (headless); human check of the SDL binary pending
**Tracking:** GitHub issue #26

## Executive Summary

`Cartridge::load_from_bytes` has always recorded iNES flags 6 bit 1
(`battery_backed`), but nothing read it. Every mapper owns an 8 KB
`prg_ram` array that is zeroed on construction, so The Legend of Zelda,
Final Fantasy, Dragon Warrior and every other battery game started from an
empty save table each launch.

PRG RAM is now written to a `.sav` file next to the ROM and read back on
the next launch. The mapper trait gained two accessor methods so `System`
can reach PRG RAM without knowing which board it is talking to, and the
persistence logic lives in two `System` methods that the binary and the
tests share.

## Design

### Mapper trait (`src/cartridge/mapper.rs`)

```rust
fn prg_ram(&self) -> Option<&[u8]> { None }
fn prg_ram_mut(&mut self) -> Option<&mut [u8]> { None }
```

Implemented by mappers 0, 1, 2, 3, 4, 5 and 65, all of which own a
`[u8; 0x2000]`. No other trait method changed.

MMC3 (`mapper4.rs`) returns the raw array regardless of its `$A001`
enable and write-protect bits. Those bits gate the CPU bus, not the
battery: a game that disables PRG RAM before jumping to its reset vector
still expects the contents to be there when it re-enables them.

MMC5 exposes only its 8 KB `prg_ram`, not ExRAM. ExRAM is not battery
backed on any known board.

### `System` API (`src/system.rs`, after the debug helpers)

```rust
pub fn load_battery(&mut self, path: &Path) -> io::Result<bool>
pub fn save_battery(&self, path: &Path) -> io::Result<bool>
```

Both are no-ops returning `Ok(false)` unless the loaded cartridge is
battery backed and its mapper reports PRG RAM.

`load_battery`:

- File missing: `Ok(false)`, RAM untouched. This is the normal first
  launch and is logged at info level by the binary, not as an error.
- File size differs from the board's PRG RAM: `log::warn!` and
  `Ok(false)`, RAM untouched. The next flush overwrites the file.
- Otherwise the file replaces PRG RAM, the hash below is recorded, and
  `Ok(true)` is returned with an info log line.
- Any other I/O error propagates.

`save_battery`:

- Hashes PRG RAM with `std::hash::DefaultHasher` and compares it with the
  hash recorded by the last successful load or save. Equal: `Ok(false)`,
  nothing written.
- Otherwise writes the whole array, records the hash and returns
  `Ok(true)` with an info log line.

### Dirty tracking

The recorded hash lives in `System::battery_saved_hash: Cell<Option<u64>>`.
`Cell` lets `save_battery` take `&self`, which keeps it callable from a
context that only holds a shared reference (an OSD or a stop hook) and
keeps the dirty state out of the emulation path entirely: no write hook
in the mapper, no per-frame comparison. Hashing 8 KB once every five
seconds is far below one frame of emulation. `Cell<u64>` is `Send`, so
`System` still travels through `Arc<Mutex<System>>` if that returns.

A hash rather than a stored copy was chosen because the comparison cost
is the same (both walk 8 KB) and the hash needs no second buffer.
`DefaultHasher::new()` is deterministic within a process, which is all
that is required; the value is never persisted.

`load_cartridge` resets the hash to `None` so the first flush after a
cartridge swap always writes.

### Binary (`src/main.rs`)

- Save path is the ROM path with its extension replaced by `.sav`
  (`Path::with_extension`), so `roms/Zelda.nes` pairs with
  `roms/Zelda.sav`.
- `load_battery` runs immediately after `load_cartridge`, before the loop.
- `save_battery` runs every `BATTERY_FLUSH_INTERVAL` (5 s) after the
  present, and once more after the loop exits. Both Quit and Escape leave
  through `break 'running`, so the post-loop call covers every exit path.
  A crash or `kill -9` loses at most five seconds of progress.
- Write failures are logged as warnings and never abort emulation.

### What is deliberately not done

- No atomic rename on write. A crash mid-write can truncate the file,
  which the next launch rejects on size and overwrites five seconds
  later. Worth revisiting if the flush interval grows.
- No save-state (whole machine) support; this is PRG RAM only.
- No configurable save directory; the sidecar file matches what most
  emulators default to and what users expect to find next to the ROM.

## Verification

### Headless (`cargo test --test battery`, 4 tests)

All four use a synthetic 32 KB NROM image (`tests/battery.rs`), write
PRG RAM through `System::poke`, read it back through `System::peek`, and
use a per-process, per-test file under `std::env::temp_dir()`.

| Test | Verifies |
|------|----------|
| `battery_ram_round_trips_between_systems` | A patterned 8 KB write, `save_battery` returning `Ok(true)`, a fresh `System` loading it with `Ok(true)`, and every byte matching. This is the two-instance round trip the issue asks for. |
| `unchanged_ram_is_not_rewritten` | Second save with no change returns `Ok(false)`; one changed byte triggers a write that lands on disk; a freshly loaded system reports nothing dirty. |
| `non_battery_rom_never_writes_or_reads` | With flags 6 bit 1 clear, `save_battery` never creates a file and `load_battery` ignores an existing one. |
| `missing_file_and_size_mismatch_are_ignored` | Missing file and a 4 KB file both return `Ok(false)` and leave RAM zero; the next save replaces the bad file with a full 8 KB. |

The full suite (`cargo test`) and `cargo clippy --all-targets -- -D warnings`
pass with the change.

### Human check still needed

Run the SDL binary with Zelda: start a file, name it, play into the first
screen, quit with Escape, relaunch. The file select screen should show
the named file. Expect these log lines with `RUST_LOG=info`:

```
No battery save at roms/Zelda.sav        (first launch only)
Wrote battery save roms/Zelda.sav (8192 bytes)
Loaded battery save roms/Zelda.sav (8192 bytes)   (second launch)
```

Final Fantasy (MMC1, battery) is a second good candidate because it
writes its save table on an explicit menu action rather than
continuously, which exercises the dirty check.

## Debugging notes

- `Ignoring battery save ...: N bytes, expected 8192` in the log means a
  `.sav` from another emulator that uses a different size or prepends a
  header. Delete it, or strip it down to the raw 8 KB PRG RAM image.
- A `.sav` that never appears: check the ROM header. `Cartridge::mapper_id`
  is logged at load; add `battery_backed` to that line if a ROM dump has
  the flag stripped. The binary only touches the file when the flag is
  set.
- To inspect a save: `xxd roms/Zelda.sav | head`. The file is the raw
  `$6000-$7FFF` image, so offset 0 is `$6000`. An all-zero file means the
  game never wrote PRG RAM, which points at the game not reaching its
  save code rather than at persistence.
