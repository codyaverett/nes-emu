# Running the Emulator in a Web Page (WebAssembly)

**Date:** 2026-09-06
**Type:** Feature Plan
**Status:** Phases 1, 2, 3 and 5 complete (docs/debugging/WASM_WEB_BUILD.md, WASM_WEB_PAGE.md, WASM_WEB_STORE.md, WASM_WEB_DEPLOY.md; Phase 5 pending its first tag deploy); Phase 4 open
**Tracking:** GitHub issues listed per phase below

## Goal

Play ROMs in a browser page with the same core, audio pacing, battery
saves, cheats, save states and tool pages as the SDL binary, deployed as
static files (GitHub Pages or any host) with no server component.

## What is already in place

- The emulator library has no SDL dependency of its own. Outside the binary
  and `src/ui/`, the only host APIs it uses are `std::fs`/`std::path` for
  battery, cheat and state files (which compile for `wasm32-unknown-unknown`
  and are simply not called there) and `Arc<Mutex<VecDeque<f32>>>` for the
  audio hand-off, which works single-threaded.
- `System::run_frame_with_audio` already produces samples at 44.1 kHz into a
  queue and the native loop paces emulation from the audio queue; the web
  loop can use the same rule.
- `save_state`/`load_state` and the cheat set are plain byte and string
  APIs, so persistence can go to IndexedDB unchanged.
- `wasm-pack` and the `wasm32-unknown-unknown` target are installed.

## What blocks it today

Building the library for wasm fails because `sdl2` is a hard dependency of
the single package and its `libc` bindings do not exist for wasm32.

## Architecture

```
nes-emu (library, unchanged API)      builds natively and for wasm32
  |
  +-- src/main.rs + src/ui/   SDL frontend, behind a `sdl` cargo feature
  |
  +-- web/                     wasm-bindgen crate + static page
        src/lib.rs             Emulator wrapper exported to JS
        index.html, app.js     canvas, audio worklet, input, storage
        pkg/                   wasm-pack output (built, not committed)
```

The web crate exports one `Emulator` object: `new(rom_bytes)`,
`run_frame()`, `frame_rgba() -> Uint8ClampedArray` (256x240x4),
`take_audio() -> Float32Array`, `set_button(player, button, down)`,
`reset()`, `save_state() -> Uint8Array`, `load_state(bytes)`,
`battery() / set_battery(bytes)`, `cheats_text() / set_cheats_text(text)`,
`rom_crc32()`. Everything else (slots, rewind ring, cheat database lookup
by CRC) lives in JS on top of those calls.

The page: a `<canvas>` scaled with `image-rendering: pixelated`, an
`AudioWorklet` fed from a ring buffer the main thread fills from
`take_audio()`, and a `requestAnimationFrame` loop that emulates until the
audio ring holds about 55 ms of samples (the native rule), so pacing is
exact regardless of display refresh. Keyboard mapping mirrors the SDL
binary. ROMs arrive by file picker or drag and drop and never leave the
browser. `.sav`, state slots and `.cht` text are stored in IndexedDB keyed
by ROM CRC-32. The bundled `cheats/` files are converted to one JSON file
at build time and fetched by the page.

Overlay UI: the palette and tool pages draw through SDL calls today. Phase
4 introduces a small `Painter` trait (`fill_rect`, `text`) used by every
tool and the palette; the SDL frontend implements it with the canvas, the
web frontend implements it by drawing into an RGBA overlay that the page
composites over the frame. One UI code path, two backends. Until then the
web page gets an HTML palette and cheat list, which is enough to play.

## Phases

### Phase 1: crate split and wasm build (#49) - done
Feature-gate the SDL frontend, add the `web/` wasm-bindgen crate with the
`Emulator` wrapper, a CI job that builds `wasm32-unknown-unknown`, and a
headless smoke test in Node or the wasm test runner that runs 60 frames of
a synthetic ROM.

### Phase 2: playable page (#50) - done (Chromium verified; Firefox and Safari need a human pass)
Canvas, AudioWorklet, audio-clocked loop, keyboard, ROM picker and drag and
drop, full-screen button, overscan crop matching the binary. Verified by
playing SMB in Chrome, Firefox and Safari; frame timing checked against
`requestAnimationFrame` counts and audio underrun counters printed in the
console.

### Phase 3: persistence, cheats and states in the browser (#51) - done (Chromium verified; Firefox and Safari need a human pass)
IndexedDB store keyed by CRC-32 for battery RAM, nine state slots and the
cheat text; the bundled cheat database as JSON; HTML controls for slots and
a cheat list with add/toggle/delete; export and import of the store as a
file so saves can move between machines. `web/storage.js`,
`web/scripts/build-cheats.mjs`, verification in
`docs/debugging/WASM_WEB_STORE.md`.

### Phase 4: shared overlay UI (#52)
`Painter` trait, SDL and RGBA implementations, palette and every tool page
running on both frontends, rewind on the web using the same ring buffer.

### Phase 5: deployment (#53) - done (pending first deploy)
GitHub Pages workflow building with `wasm-pack build --release --target
web`, a size budget (target under 500 KB of wasm), and a hosted demo link
in the README. Users supply their own ROMs; nothing copyrighted is hosted.

## Risks and notes

- Audio in browsers requires a user gesture before the AudioContext starts;
  the page must show a "click to start" gate.
- `AudioWorklet` needs the page served over HTTPS or localhost; the plain
  `file://` open will fall back to `ScriptProcessorNode` or silence.
- wasm has no threads here; the emulator is single-threaded already.
- Expect roughly a 3x to 5x slowdown versus native release. Native runs at
  several hundred frames per second, so 60 fps has ample margin.
- `std::fs` calls in the library must never run on wasm; the web wrapper
  uses the byte and string APIs only. A `cfg(target_arch)` guard is not
  required, but a test that the wrapper compiles without them is.
- Save-state files are build-bound (layout changes refuse old states); the
  web store should record the emulator version with each state.
