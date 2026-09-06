# Shared overlay UI: Painter, Key and Host (issue #52)

**Date:** 2026-09-06
**Type:** Refactor / Web feature
**Status:** Complete headlessly; the feel of the web overlay, audio on
rewind and the non-Chromium browsers need a human pass
**Tracking:** GitHub issue #52 (Phase 4 of `docs/plans/WASM_WEB.md`),
sub-issues #58 (harness, Painter, Key), #59 (Host, clock, library
move), #60 (wasm wrapper), #61 (page and docs); plan in
`docs/plans/SHARED_OVERLAY_UI.md`

## Executive summary

`src/ui/` (command palette, the six tool pages, toasts, rewind, the
shared hotkeys) moved from the SDL binary into the `nes_emu` library
behind three small abstractions: `Painter` (a surface with `size` and
`fill_rect`), `Key` (a frontend-neutral key press) and `Host` (state
slots and the cheat text). The SDL binary wraps its `WindowCanvas` in
`SdlPainter`, maps `Keycode` to `Key` and keeps `<rom>.sN` / `<rom>.cht`
through `FileHost`; the web page draws the same UI into an RGBA overlay
canvas, maps `KeyboardEvent.code` through `Key::from_browser_code` and
mirrors nine in-memory slots to IndexedDB through `WebHost`'s dirty
flags. The HTML toast element is gone: toasts, the volume bar and the
paused reminder draw through the same `draw_messages` on both.

The SDL output is pixel-identical: a screenshot harness
(`scripts/ui_screenshots.sh`) hashed 15 captures on the untouched tree
and every later phase reproduced the same hashes.

## Architecture

```
src/ui/painter.rs   Color, Painter { size, fill_rect }, RgbaPainter (src-over blend, clipping)
src/ui/key.rs       Key { Char(c), Backquote, Escape, Return, ..., F1-F8, Other },
                    printable(), digit(), from_browser_code(code)
src/ui/host.rs      SlotInfo, Host { write_state, read_state, slot_info, slot_label,
                    write_cheats, cheats_label }, FileHost (cfg not wasm32)
src/ui/app.rs       App { host: Box<dyn Host>, now_ms, ... }, rewind_frame, messages_visible
src/ui/mod.rs       Ui::key_down / key_up (UI first, then the shared hotkeys), is_active,
                    draw_messages (volume bar, toast, paused reminder), WINDOW_SCALE, overlay_size
src/main.rs         SdlPainter, sdl_key(Keycode) -> Key, FileHost, Escape quits, controller map
web/src/host.rs     WebHost over a SlotStore shared with the Emulator (Rc<RefCell>), dirty flags
web/src/lib.rs      Emulator { app, ui, painter } and the exports listed below
web/app.js          overlay canvas, keys through emu.key_down, per-tick clock, rewind branch,
                    control sync, slot and cheat persistence from the dirty flags
```

Key routing on both frontends: `Ui::key_down(key, app)` gives the key
to the palette or the open page (which consume every key, so an
unmapped key never leaks to the controller while a page is open), then
to the shared hotkeys (F1, R, P, N, M, plus, minus, F5-F8, Backspace).
It returns true when the key was used; only then does the SDL binary
skip the controller map and the page skip its controller table and
`preventDefault` (so F5 saves a state instead of reloading the page).
Escape in game mode is the frontend's: the binary quits, the page
ignores it. F (fullscreen) stays in JavaScript. Releases always reach
the controller.

The clock: `App::now_ms` is set by the frontend before input and
drawing (`Instant` since start in the binary, `Date.now()` on the web);
`show_message` stamps toasts with `now_ms + 2000` and the rewind line
with `now_ms + 500`. Nothing under `src/ui/` reads `Instant`,
`SystemTime`, `std::fs` or `sdl2` outside the `cfg(not(wasm32))`
`FileHost`, which is what makes `cargo build --no-default-features` and
the wasm build carry the whole UI.

The web overlay is `visible_size * WINDOW_SCALE` (720x672 with the
crop, 768x720 without), the SDL window size, drawn into an
`RgbaPainter` and uploaded to a second canvas layered over the frame
with `putImageData`. `overlay_visible` is true while the palette or a
page is open, a toast or the volume bar is showing, or the paused
reminder applies; when it turns false the page clears the canvas once.

Web exports added to `Emulator`: `key_down(code) -> bool`,
`key_up(code)`, `set_now_ms(ms)`, `tick()`, `overlay_size() -> [w, h]`,
`overlay_visible()`, `overlay_rgba()`, `osd_message(text)`, `paused`,
`set_paused`, `take_frame_advance`, `muted`, `toggle_mute`, `volume`,
`crop_enabled`, `set_crop`, `ui_active`, `rewinding`, `rewind_step`,
`rewind_seconds`, `slot`, `set_slot`, `save_slot`, `load_slot`,
`set_slot_cache(slot, bytes, modified_ms)`, `clear_slot_cache`,
`take_dirty_slots() -> [slot]`, `slot_bytes(slot)`,
`take_cheats_dirty()`, `osd_text()`. `run_frame` now records rewind
snapshots.

## The screenshot harness (Phase 0)

`scripts/ui_screenshots.sh ROM OUTDIR` copies the ROM into OUTDIR,
writes `<rom>.cht` (`SXIOPO`) and `<rom>.s1..s3` (15098 zero bytes)
with `TZ=UTC touch -t 202601010000`, rebuilds the binary and runs two
scripted sessions with `--no-audio`: the documented palette/help one,
and one that visits every page from the title screen (90 empty frames
first), a `Slot 2 (saved)` toast after F7, a `Backspace*40` rewind and
the Help page after it. It prints `shasum -a 256` of the 15 PPMs to
OUTDIR/hashes.txt. The scripts only view and navigate; nothing writes
to OUTDIR during a run, so the States and Cheats pages are stable.

Baseline on the untouched tree (v0.14.0 plus the plan), two runs,
identical hashes:

| Capture | SHA-256 (first 16) |
| --- | --- |
| palette.ppm | 5eeb88e3052e2f88 |
| help.ppm | fd9fbe8da28f0e5a |
| memory.ppm / memory_page2.ppm | bab1b14044014684 / ec99d2335e5d631d |
| ppu_patterns / ppu_nametables / ppu_palettes | b63312eac4dd2ca7 / c8a14340f919f480 / 5a25be4b8bb8a805 |
| apu.ppm / apu_unmuted.ppm | 5e5c2c82a0435231 / b3948b7330895fa2 |
| cheats.ppm | 7b2f12f32d9f4b84 |
| states.ppm / states-cursor.ppm | 96d61697821a9c45 / 78e44999412dc94a |
| toast.ppm | ab74abe0cdbdb2e1 |
| rewind.ppm | f0e3ba41247700da |
| help-rewind.ppm | a490a65f8338648a |

The same 15 hashes came back after Phase 1 (Painter), Phase 2 (Key)
and Phase 3 (Host, clock, library move). The committed PNGs under
`docs/testing/test_output/ui/` were not regenerated.

## Verification

| Check | How | Result |
| --- | --- | --- |
| Baseline stable | two harness runs on the untouched tree | 15/15 hashes equal |
| Painter keeps pixels | harness after Phase 1 | 15/15 equal to baseline |
| Key keeps pixels | harness after Phase 2 | 15/15 equal to baseline |
| Host and clock keep pixels | harness after Phase 3 | 15/15 equal to baseline |
| RgbaPainter | unit tests: opaque fill, translucent over transparent keeps the source alpha, blend over opaque, clipping, clear and resize | pass |
| Key mapping | unit tests for browser codes (letters, digits, punctuation, named keys, unknown to Other) and SDL codes; the two agree on shared keys | pass |
| Hotkeys and consumption | unit tests: P pauses, F7 steps the slot, Backspace rewinds; Z, Escape, F, Return are not consumed in game mode; every key is consumed with the palette open | pass |
| FileHost | unit tests: labels `game.s3` / `game.cht`, slot round trip, metadata, write errors | pass |
| App through a memory host | unit tests: toast expiry against now_ms, slot save/load/set messages, cheat rewrite, rewind OSD | pass |
| No SDL library | `cargo build --no-default-features` | pass |
| wasm | `cargo build -p nes-emu-web --target wasm32-unknown-unknown`; wasm-pack 277 KB of 500 KB budget | pass |
| WebHost | unit test: cache reads are not dirty, UI writes are, stamps, cheats flag | pass |
| Emulator (native tests) | keys, palette overlay alpha, toast expiry, crop resize 720x672 / 768x720, rewind, F5 dirty slot, palette `cheat add sxiopo` marks cheats dirty | 7 pass |
| Node smoke (`npm test`) | Backquote opens, overlay 720x672 with alpha present, typing consumed, F1, rewind_step consumes snapshots, F5 dirties slot 1, F7/F8, toast visible and readable through osd_text | pass |
| Page: ROM load | Chromium headless, `nesLoadRom(mario.nes)` from a CORS server | mapper 0, CRC 8E2BD25C, audio running, `Loaded mario.nes` toast (23040 alpha pixels), no `#toast` element |
| Page: palette | dispatched `Backquote` keydown | `uiActive` true, 168960 alpha pixels, game keeps running underneath, `P` typed into it does not pause |
| Page: Escape | dispatched `Escape` | overlay cleared (0 alpha) |
| Page: Help | `F1` | 483840 alpha pixels (the full 720x672 page); Escape clears |
| Page: pause | `P` | frames hold, button reads Resume, toast drawn; `P` resumes |
| Page: rewind | hold `Backspace` 700 ms after 1.5 s of play | `rewinding` true, frame counter held at 2338, frame hash 6fc5174d -> b14e7fe4, buffered 20.0 s -> 18.6 s, `Rewinding` toast; release resumes |
| Page: F5 | `F5` | slot 1 written to IndexedDB (15098 bytes, core 0.14.0) by the dirty flag, list reads `Slot 1 2026-09-06 01:22`, toast drawn |
| Page: F7 | `F7` | core slot 2, list highlight follows |
| Page: reload keeps the slot | `page.goto`, load the ROM again | store lists slot 1; cache holds 15098 bytes; States page shows it; `F8` loads (frame changes, toast) |
| Page: screenshots | composite of frame and overlay via `toDataURL`, posted to a scratch server | `docs/testing/test_output/ui/web-palette.png` (720x672) matches the layout of `palette.png` from the SDL harness |
| Console | Playwright console log | no errors; two Canvas2D `willReadFrequently` warnings from the test hook's `getImageData` |
| `web/test/browser-store.mjs` (issue #51 script, not part of `npm test`) | `NODE_PATH=<playwright> node test/browser-store.mjs` against the two local servers with zelda.nes and mario.nes | all store checks passed: battery flush and restore, slot round trip (frame hash d9e19dc5 before and after), `Slot 5 (empty)` toast through `osd_text`, F6/F7/F5/F8, buttons, cheats seed/edit/reload/delete, export and import |

Not verified headlessly (needs a human): audio while rewinding and on
release (the worklet queue is cleared when the rewind starts), the feel
of the overlay scaling on a large or non-integer CSS size, fullscreen,
Firefox and Safari, the physical keyboard typing `:` as `;` in the
Cheats page, and the volume bar appearance on the web (the page starts
at unity gain, the binary at 50%).

## Debugging steps

1. **Harness determinism.** The States page prints the slot mtime in
   UTC while `touch -t` uses local time, so the fixtures are touched
   with `TZ=UTC`; the binary seeds `<rom>.cht` from `cheats/` by CRC
   when none exists and rewrites it on every edit, so the harness
   writes a fixed `.cht` before each run and scripts no A/S/F5. Two
   runs agreed on the first try.
2. **`RgbaPainter` dead in the binary.** Until Phase 3 nothing in the
   binary constructed it, and clippy runs with `-D warnings`; it and
   `Key::from_browser_code` carried `#[allow(dead_code)]` for two
   commits and lost it when the module became a public library API.
3. **BSD sed.** `\s` in the bulk rename is not supported by macOS sed;
   the second pass used `[[:space:]]`, and the `Keycode` to `Key`
   rewrite ran as a Python script instead.
4. **`Tab` was missing from the plan's `Key` enum** but the PPU page
   uses it to switch views; it was added (and mapped from the browser's
   `Tab` code) rather than losing the key.
5. **`src_rect` shadowing.** The `sdl2::Rect` helper moved from `App`
   to `main.rs` as `fn src_rect`, which the existing `let mut src_rect`
   shadowed (`expected function, found Rect`); renamed `source_rect`.
6. **`load_state` in the App tests needed a cartridge**: the first
   run failed with `no cartridge loaded`; the test helper now loads a
   synthetic NROM like the PPU page tests do.
7. **Rewind snapshot timing log** measured the `save_state` cost with
   `Instant` inside `record_rewind_frame`, which compiles for wasm but
   panics at runtime. The log now reports the size only; the measured
   cost from issue #44 stays in `UI_FRAMEWORK.md`.
8. **Web slot timestamps.** `Host::write_state` has no clock argument,
   so the page passes `Date.now()` as `now_ms` and the `Emulator`
   forwards the seconds to the `SlotStore`, which stamps writes; the
   page's IndexedDB record keeps its own `at` as before.
9. **F5 must `preventDefault` when it saves, not only when a page is
   open.** `Ui::key_down` therefore returns true for a used hotkey as
   well as for a consumed key, and the page skips the controller table
   in both cases (no hotkey shares a controller key, so the binary's
   behaviour is unchanged).
10. **The page's toast helper is called before a ROM exists** (store
    errors at boot); with no `Emulator` there is no overlay, so those
    go to the console instead.
11. **`web/test/browser-store.mjs` read the `#toast` element** (three
    places: the empty-slot message, the `Loaded slot 2` wait, the cheat
    add) and the element is gone. The wrapper gained `osd_text()` (the
    toast the core is showing) and the page `nesApp.osdText()`; the
    script uses those. The script is not part of `npm test`; it was
    re-run against the branch after the change (see Verification).
12. **Screenshot capture.** The Playwright MCP screenshot saves to its
    own directory and captures the CSS-scaled element, so the page
    composites frame plus overlay into an offscreen canvas at overlay
    size and `POST`s the PNG to the scratch ROM server (a
    `SimpleHTTPRequestHandler` with CORS headers and a `/save/` route).

## Deviations from the plan

- `Key::Tab` added (item 4 above).
- `Ui::key_down` returns true for hotkeys too (item 9).
- `Emulator` has a few exports beyond the plan's list so the page's
  buttons go through the core (`toggle_mute`, `set_crop`, `set_slot`,
  `save_slot`, `load_slot`, `clear_slot_cache`, `rewind_seconds`,
  `tick`).
- CHANGELOG and the version are left for the merge, per the task; the
  plan's Phase 6 wording predates that decision.
