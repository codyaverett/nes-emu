# nes-emu-web

wasm-bindgen wrapper and browser page for the emulator core
(`docs/plans/WASM_WEB.md`). The wrapper (`src/lib.rs`) exposes one
`Emulator` object; `index.html`, `app.js` and `audio-worklet.js` are the
page (`docs/debugging/WASM_WEB_PAGE.md`).

## Play

```sh
npm run build && npm run serve   # then open http://127.0.0.1:8080/
```

Click the screen or drop a `.nes` file on it. Keys match the SDL binary
(listed under the canvas). Stats under the canvas show emulated fps,
display rate, queued audio and underruns; `window.nesStats` holds the
same numbers for scripts.

## Build

```sh
# from this directory
npm run build        # wasm-pack build --release --target web  -> pkg/
npm test             # Node build (pkg-node/) plus test/smoke.mjs, 60 frames
npm run serve        # static server on http://127.0.0.1:8080 (AudioWorklet needs a secure context)

# from the repository root, without wasm-pack
cargo build -p nes-emu-web --release --target wasm32-unknown-unknown
cargo test -p nes-emu-web   # the wrapper's own tests, run natively
```

`wasm-pack` downloads a matching `wasm-bindgen` CLI on first use.
`pkg/` and `pkg-node/` are build outputs and are ignored by git.

## Notes

- The core is about 190 KB by value; `.cargo/config.toml` at the
  repository root raises the wasm stack to 8 MB so its constructor does
  not fault (the default 1 MB stack overflowed with "memory access out
  of bounds").
- Errors are `Result<_, String>` so the same code runs in host tests;
  wasm-bindgen turns them into thrown JS strings.
