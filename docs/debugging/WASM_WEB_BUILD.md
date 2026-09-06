# WebAssembly build of the core (issue #49)

**Date:** 2026-09-06
**Tracking:** GitHub issue #49 (Phase 1 of `docs/plans/WASM_WEB.md`)

## What changed

- `sdl2` and `env_logger` are optional dependencies behind a default
  `sdl` feature; the `nes-emu` binary declares `required-features =
  ["sdl"]`. `cargo build`, `cargo test`, `cargo run --bin nes-emu` behave
  as before. `cargo test --no-default-features` proves the library and
  every integration test compile and pass without SDL.
- The repository is now a workspace with one member, `web/`
  (`nes-emu-web`, a `cdylib` + `rlib`). `cargo build -p nes-emu-web
  --target wasm32-unknown-unknown` compiles only the core and the wrapper;
  the build log shows no `sdl2` crate.
- `System` gained a byte-level battery API for frontends without a file
  system: `battery_ram`, `set_battery_ram`, `battery_dirty`,
  `mark_battery_saved` (tests in `tests/battery.rs`).
- `web/src/lib.rs` exports `Emulator`: `new(rom)`, `run_frame`,
  `frame_rgba` (256x240 RGBA, `Uint8ClampedArray`), `take_audio`
  (44.1 kHz f32), `queued_audio`, `set_button(player, Button, down)`,
  `release_all`, `reset`, `save_state`/`load_state`, `battery`/
  `set_battery`/`battery_dirty`/`mark_battery_saved`, `cheats_text`/
  `set_cheats_text`, `rom_crc32`, `mapper_id`, `battery_backed`, and a
  free function `core_version`.
- `web/test/smoke.mjs` runs 60 frames of a synthetic NROM through the
  Node build and checks frame size, alpha, audio sample count, state and
  cheat round trips. `npm test` in `web/` builds and runs it.
- `.github/workflows/ci.yml`: native gates with `libsdl2-dev`, a
  `--no-default-features` test job, and a wasm job (cargo build for
  wasm32, wasm-pack, Node smoke test, module size). Written in this
  change but not yet exercised on GitHub.

## Debugging steps

1. A first probe (a scratch crate depending on `nes-emu` by path) failed
   to compile for wasm32 inside `sdl2`'s `libc` bindings. Making `sdl2`
   optional fixed the compile; `cargo test --no-default-features` passed
   all 210 unit tests and the integration suites unchanged.
2. Audit of `src/` (excluding `main.rs`, `ui/`, `bin/`) for host APIs
   that compile but panic on wasm (`Instant`, `SystemTime`, `thread`,
   `std::env`): only `src/system/tests.rs` uses them, which is fine.
   `std::fs` is used by the path-based battery and cheat loaders, which
   the wrapper never calls.
3. First Node run: every `new Emulator(...)` call, including the
   too-small-ROM error path, threw `RuntimeError: memory access out of
   bounds` from the constructor. `size_of::<System>()` is 191 280 bytes
   (the PPU's frame buffer and tables dominate). The constructor builds
   `System` by value, so the prologue reserves several copies of it and
   overflows the 1 MB default wasm stack before the ROM is even
   inspected. Fix: `.cargo/config.toml` sets `-zstack-size=8388608` for
   `wasm32-unknown-unknown`. After that the smoke test ran 60 frames in
   83 ms (about 725 fps) in Node.
4. `[profile.release]` in `web/Cargo.toml` was ignored with a warning
   (profiles apply only at the workspace root). The size-over-speed
   setting moved to the root as `[profile.release.package.nes-emu-web]
   opt-level = "s"`; the native binary keeps its profile. The optimised
   module is 164 KB after `wasm-opt`.

## Verified / unverified

- Verified locally: all four native gates, `cargo test
  --no-default-features`, `cargo test -p nes-emu-web`, wasm32 cargo build,
  `wasm-pack` Node build and smoke test.
- Unverified: the GitHub Actions workflow has not run yet; a browser
  page does not exist until issue #50.
