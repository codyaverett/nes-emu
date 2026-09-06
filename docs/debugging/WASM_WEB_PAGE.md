# The playable web page (issue #50)

**Date:** 2026-09-06
**Tracking:** GitHub issue #50 (Phase 2 of `docs/plans/WASM_WEB.md`)

## What was built

`web/index.html`, `web/app.js` and `web/audio-worklet.js` on top of the
Phase 1 wrapper (`docs/debugging/WASM_WEB_BUILD.md`). No framework, no
bundler: `npm run build` produces `web/pkg/` with wasm-pack and any
static server can host the directory.

- **Canvas.** 240x224 by default, the 8 pixel overscan crop of the SDL
  binary, drawn with `putImageData(frame, -8, -8)` (it clips, ignores
  transforms) and scaled by CSS with `image-rendering: pixelated`. The
  "Full frame" button switches to 256x240 like `--full-frame`.
- **Audio.** An `AudioWorklet` (`nes-audio`) holds a 32 768 sample ring.
  The main thread posts each frame's samples from `take_audio` with a
  transferred buffer. On underrun the worklet repeats the last sample
  (the SDL callback rule) and, every eight `process` calls, reports
  `consumed`, `queued`, `underruns` and `dropped` back. No
  `SharedArrayBuffer`: it needs cross-origin isolation headers that
  static hosts such as GitHub Pages do not send.
- **Pacing.** Audio is the master clock, as in `src/main.rs`. Each
  `requestAnimationFrame` tick emulates while the estimated queue is
  below 2450 samples (`AUDIO_TARGET_SAMPLES`, about 55 ms), at most four
  frames per tick. The estimate is `sent - (consumed + elapsed *
  sampleRate)` from the worklet's last report, so a 120 Hz display or a
  throttled tab does not change the game speed. Without a working
  AudioContext the page runs one frame per tick.
- **Input.** The SDL key map: Z/X/Right Shift/Enter/arrows for player 1,
  quote/semicolon/comma/period/IJKL for player 2; R reset, P pause, M
  mute, F fullscreen. All buttons release on window blur and when the
  tab is hidden. Since issue #52 every key goes to the core's
  `key_down` first (palette, pages, hotkeys) and the controller table
  only sees what it did not use (`docs/debugging/SHARED_OVERLAY_UI.md`). The gate accepts a click (file picker) or a dropped
  `.nes`; nothing is uploaded anywhere.
- **Diagnostics.** `window.nesStats` (frames, display and emulated fps,
  queued samples, underruns, dropped samples, audio state) is updated
  every tick, printed under the canvas once a second and logged with a
  `[nes]` prefix every five seconds. `window.nesLoadRom(bytes, name)`
  loads a ROM without the file picker, for scripts.

## Verification (headless Chromium through Playwright)

Served `web/` on `http://127.0.0.1:8080` (a secure context, so the
worklet loads) and the ROM from a second local server with a CORS
header, then drove the page with `page.evaluate`:

| Check | Result |
|-------|--------|
| Load `mario.nes` | mapper 0, CRC 8e2bd25c, gate hidden, canvas 240x224 |
| 10 s of play | emulated 60.0 fps, display 60 Hz, queue 2500-2800 samples, 0 underruns, 0 dropped |
| Synthetic Enter, hold Right, tap Z | Mario runs and jumps in 1-1 (`docs/testing/test_output/web/smb-chromium.png`) |
| P, P | frame counter holds while paused; on resume the queue refills to target within one tick |
| Full frame button | canvas 256x240, back to 240x224 |
| M | gain 0 then 1, toast text updates |
| AudioContext | running at 44 100 Hz, base latency 5.4 ms |

Not verified: Firefox and Safari (the Playwright server here is
Chromium only), real speakers, and fullscreen (headless has no
fullscreen). Those need a human pass: open the page, load a ROM, listen
for crackle and watch the underrun counter under the canvas.

## Debugging steps

1. **Pacing ran at 240 fps with the queue reading 0** and the worklet
   dropping hundreds of thousands of samples. `nesAudio.sent` stayed at
   0: the chunk was posted with its buffer in the transfer list, and
   `samples.length` was read after the post, when the buffer was already
   detached and the length 0. Fix: read the length before posting.
   Verified 60.0 fps, 0 dropped afterwards.
2. **161 underruns after a one second pause.** While paused nothing is
   sent, so the worklet starved on every `process` call. That is
   silence, not starvation, so the worklet now counts underruns only
   after samples have arrived since the last `clear`, and pause as well
   as resume sends `clear`. Verified 0 underruns across pause and resume.
3. Playwright's file upload helper waits for the `<input type=file>` to
   be visible and the page hides it behind a styled label. Rather than
   change the page for the tool, `window.nesLoadRom` takes bytes; the
   test fetched the ROM from a scratch server with
   `Access-Control-Allow-Origin: *`.
