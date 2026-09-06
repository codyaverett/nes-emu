# nes-emu-web

wasm-bindgen wrapper and browser page for the emulator core
(`docs/plans/WASM_WEB.md`). The wrapper (`src/lib.rs`) exposes one
`Emulator` object; `index.html`, `app.js` and `audio-worklet.js` are the
page (`docs/debugging/WASM_WEB_PAGE.md`) and `storage.js` is its
IndexedDB store (`docs/debugging/WASM_WEB_STORE.md`).

## Play

```sh
npm run build && npm run serve   # then open http://127.0.0.1:8080/
```

Click the screen or drop a `.nes` file on it. Keys match the SDL binary
(listed under the canvas): F5 saves a state, F8 loads it, F6/F7 change
the slot, Backspace held rewinds, backquote opens the command palette
and F1 the Help page. The palette and its pages (cheats, memory, ppu,
apu, states) are the binary's own, drawn by the core into an overlay
canvas (`docs/debugging/SHARED_OVERLAY_UI.md`); toasts appear there
too. Stats under the canvas show emulated fps, display rate, queued
audio and underruns; `window.nesStats` holds the same numbers for
scripts.

## Saves, states and cheats

Everything is stored in the browser's IndexedDB, keyed by the ROM's
CRC-32, and never leaves the machine:

- Battery RAM is written every 5 s when it changed, when the tab is
  hidden or closed, and by the "Save battery" button.
- Nine state slots (Save/Load/Delete buttons or F5/F8) record the time
  and the core version that wrote them.
- The cheat list accepts the binary's code syntax: Game Genie (6 or 8
  letters), `AAAA:VV`, `AAAA?CC:VV`, parts joined with `+`. On the first
  load of a ROM the list is seeded from the bundled database
  (`cheats.json`, built from `../cheats/*.cht` by `npm run build`) with
  every code off; the first edit stores your own copy.
- "Export all" downloads the whole store as one JSON file; "Import"
  merges such a file back, so saves can move between machines.

## Build

```sh
# from this directory
npm run build        # cheats.json from ../cheats, then wasm-pack build --release --target web -> pkg/
npm test             # node --test unit tests, Node build (pkg-node/) plus test/smoke.mjs
npm run check-size   # wasm size budget (500 KB) and debug-info check, docs/debugging/WASM_WEB_DEPLOY.md
npm run serve        # static server on http://127.0.0.1:8080 (AudioWorklet needs a secure context)

# from the repository root, without wasm-pack
cargo build -p nes-emu-web --release --target wasm32-unknown-unknown
cargo test -p nes-emu-web   # the wrapper's own tests, run natively
```

`test/browser-store.mjs` drives the page in headless Chromium through
Playwright (not part of `npm test`; see the file header for the servers
it needs).

`wasm-pack` downloads a matching `wasm-bindgen` CLI on first use.
`pkg/`, `pkg-node/` and `cheats.json` are build outputs and are ignored
by git.

A hosted build is deployed to https://codyaverett.github.io/nes-emu/ on
every version tag by `.github/workflows/pages.yml`.

## Notes

- The core is about 190 KB by value; `.cargo/config.toml` at the
  repository root raises the wasm stack to 8 MB so its constructor does
  not fault (the default 1 MB stack overflowed with "memory access out
  of bounds").
- Errors are `Result<_, String>` so the same code runs in host tests;
  wasm-bindgen turns them into thrown JS strings.
- Save states are build-bound; a slot written by another core version
  shows that version in the list and may be refused on load.
