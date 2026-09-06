# Browser persistence: battery RAM, state slots and cheats (issue #51)

**Date:** 2026-09-06
**Tracking:** GitHub issue #51 (Phase 3 of `docs/plans/WASM_WEB.md`)

## What was built

The page from Phase 2 (`docs/debugging/WASM_WEB_PAGE.md`) now keeps
what the SDL binary keeps next to the ROM (`.sav`, `.s1`..`.s9`,
`.cht`) in IndexedDB, keyed by the ROM's CRC-32. Nothing in the
wrapper (`web/src/lib.rs`) changed; the store is JavaScript on top of
`battery/set_battery/battery_dirty/mark_battery_saved`,
`save_state/load_state`, `cheats_text/set_cheats_text`, `rom_crc32`
and `core_version`.

### `web/storage.js`

One IndexedDB database (`nes-emu`, version 1) with four object stores,
so every write is a single `put` and never a read-modify-write:

| store     | key           | record                                         |
|-----------|---------------|------------------------------------------------|
| `roms`    | `crc`         | `{ crc, name, seenAt }` (last file name seen)   |
| `battery` | `crc`         | `{ crc, bytes: Uint8Array, at }`               |
| `states`  | `"CRC:slot"`  | `{ key, crc, slot, bytes, at, core }` + index on `crc` |
| `cheats`  | `crc`         | `{ crc, text, at }` (the `.cht` text)          |

`crc` is eight upper-case hex digits (`crcKey(emu.rom_crc32())`), the
same form the bundled cheat files declare in their `# crc32:` lines.
`openStore()` returns a promise API (`getBattery/setBattery/
setBatterySync`, `getState/setState/deleteState/listStates`,
`getCheats/setCheats/deleteCheats`, `dump/exportJson/importJson/clear`).
The module touches `indexedDB` only inside `openStore`, so Node can
import its pure helpers (`crcKey`, base64, `encodeExport`,
`decodeExport`) for `node --test`.

### `web/app.js`

- **ROM load.** The new `Emulator` is built into a local, the old
  one's battery RAM is flushed and freed, then battery RAM and cheats
  for the CRC are restored from the store before the local becomes the
  live `emu`. The animation loop emulates as soon as `emu` is set, and
  Zelda writes its SRAM on boot, so restoring after would race.
- **Battery.** A 5 s interval calls `flushBattery()` which writes only
  when `battery_dirty()`; `pagehide`, `beforeunload` and the tab going
  hidden flush too. The unload path uses `setBatterySync`, which issues
  the `put` synchronously inside the handler (anything after an `await`
  there is lost). `set_battery` clears the dirty flag, so the "Save
  battery" button and `nesApp.flushBattery(true)` force a write.
- **States.** Nine slots as a clickable list showing the save time and,
  when it differs from the running core, the version that wrote the
  slot. F5 saves, F8 loads, F6/F7 step the slot with wrap-around
  (`App::prev_slot/next_slot`); Save, Load and Delete buttons do the
  same. Loading an empty slot or a state the core refuses shows the
  error in a toast. Each record carries `core_version()` because state
  files are build-bound (`docs/debugging/SAVE_STATES.md`).
- **Cheats.** The list is always re-read from `emu.cheats_text()` after
  every change (the wrapper normalises codes: upper case, dashes and
  spaces stripped). Add, toggle and delete rebuild the `.cht` text, hand
  it to `set_cheats_text` and, on success, store `cheats_text()` under
  the CRC. A bad code leaves the set untouched and the wrapper's error
  is shown (minus its `line N:` prefix). The form accepts every syntax
  the binary does: Game Genie 6/8 letters, `AAAA:VV`, `AAAA?CC:VV`,
  parts joined with `+`.
- **Bundled database.** `npm run build` runs
  `web/scripts/build-cheats.mjs`, which converts `cheats/*.cht` into
  `web/cheats.json` (`{ "<CRC>": { name, text } }`, one entry per
  `# crc32:` line, first file in name order wins like
  `find_in_database`). When a ROM has no stored cheat text the page
  fetches the file once and seeds the set from the CRC match with every
  code forced off. The seed is **not** written to the store: the first
  edit stores the user's copy, so an unedited game keeps following
  database updates after a rebuild. The note under the list says which
  state it is in. `cheats.json` is a build output and gitignored.
- **Export and import.** "Export all" downloads
  `nes-emu-store-<stamp>.json` (`{ format: "nes-emu-web-store",
  version: 1, exportedAt, roms: { CRC: { name, battery, cheats,
  states } } }`, bytes base64). Import merges: records with the same key
  are replaced, everything else is kept, and the loaded ROM's battery
  RAM and cheats are re-applied so imported saves take effect without a
  reload. Foreign files are rejected by `decodeExport`.
- **Keys and text fields.** The controller mapping used to see every
  keydown; typing `z` in the cheat form would press A and `p` would
  pause. The handlers now ignore events whose target is a text input.
  F5-F8 are only claimed while a ROM is loaded, so an empty page still
  reloads on F5.
- **Hooks.** `window.nesStats` gained `crc`, `slot`, `batteryFlushes`
  and `cheatsSeeded`. `window.nesStore` is the storage API and
  `window.nesApp` exposes `emu`, `flushBattery`, `saveState/loadState/
  deleteState/setSlot/prevSlot/nextSlot`, `cheats/addCheat/toggleCheat/
  deleteCheat/applyCheats`, `exportJson/importStore`, `runFrames`,
  `frameHash` and `setPaused` for scripts.

## Verification

Unit tests (`cd web && npm test`, also run by CI):
`test/build-cheats.test.mjs` (CRC extraction, name, duplicate CRCs,
every bundled file has a CRC and ships disabled) and
`test/storage.test.mjs` (crcKey, base64 up to 200 KB, export shape,
JSON round trip, rejection of foreign files); 10 tests.

Headless Chromium (`web/test/browser-store.mjs`, Playwright 1.57,
one browser context so IndexedDB survives `page.goto`). Served `web/`
on `127.0.0.1:8791` and the ROMs from a scratch directory of symlinks
on `127.0.0.1:8766` with `Access-Control-Allow-Origin: *`; the page
fetched the bytes and called `window.nesLoadRom`. ROMs stay outside the
repository.

| Check | Result |
|-------|--------|
| (a) Zelda (mapper 1, CRC 3FE272FB): `set_battery` pattern, forced flush | stored 8192 bytes, hash equals live RAM |
| (a) 5 s dirty-flag flush while Zelda runs 6.5 s | 1 flush in 393 frames (the game initialises its SRAM at boot), dirty cleared |
| (a) reload page, reload ROM | battery hash `1d0b28a3` restored, 0 frames run before the check |
| (b) save slot 1, run 120 frames, load slot 1 | frame hash `d9e19dc5` before and after (moved frame `409c4a75`); record 23299 bytes, core 0.13.0 |
| (b) slot list, empty slot | "Slot 1 2026-09-06 00:29"; loading slot 5 toasts "Slot 5 is empty" |
| (b) keys | F6 from 1 wraps to 9, F7 F7 reaches 2, F5 saves slot 2, F8 loads it; used slots [1, 2] |
| (b) buttons | click slot 3, Save fills it, Delete empties it |
| (c) mario.nes (CRC 8E2BD25C) first load | seeded from cheats.json: 26 codes, 0 enabled, SXIOPO first, nothing stored yet |
| (c) checkbox enables SXIOPO, form adds `075a:02` "two coins" | `cheats_text` has `SXIOPO\t1` and `075A:02\t1\ttwo coins`; stored text equals it; code box cleared |
| (c) bad code `NOTACODE!` | returns "invalid Game Genie letter 'C' (valid: APZLGITYEOXUKSVN)", set unchanged |
| (c) reload page, reload ROM | `cheats_text` identical, `cheatsSeeded` false |
| (c) delete | 075A:02 gone from text and store |
| (d) export, clear store, import | dump (CRCs, byte hashes, cores, cheat text) identical before and after; 2 ROMs, 1 battery, 2 states, 1 cheat file |
| (d) foreign JSON | rejected, toast shown |
| (d) Export button | Playwright `download` event, `nes-emu-store-2026-09-06-05-26-26.json` |

Not verified headlessly: the file picker for import (Playwright cannot
see the hidden `<input type=file>`; the import path is exercised
through `nesApp.importStore` with the same text the picker would
read), Firefox and Safari IndexedDB behaviour, and whether the
`pagehide` flush lands when the tab is closed by hand. Those need a
human pass: play Zelda until the save screen, close the tab, reopen
and reload the ROM; import a file exported from another machine.

## Debugging steps

1. **The shared Playwright MCP browser changed URL mid-check.** After
   the battery check the page suddenly sat at
   `http://127.0.0.1:8913/nes-emu/` with no ROM: the MCP server's single
   browser is shared with other sessions, and another one navigated it.
   Rather than fight for the tab, the checks moved to
   `web/test/browser-store.mjs`, a Node script driving its own
   Playwright Chromium (the MCP's `playwright` package under
   `NODE_PATH`). Its context is private, so page reloads keep IndexedDB
   and nothing else touches the page.
2. **Port 8765 was already taken** by an unrelated local service (the
   bind failed with "Address already in use"); the page server moved to
   8791.
3. **F6 landed on slot 4 instead of 9.** The script had just called
   `loadState(5)` to check an empty slot, which selects the slot like
   the binary's F8 does, so F6 stepped 5 to 4. The test resets the slot
   to 1 first; the page behaviour is the intended one.
4. **Bad-code errors read "line 28: invalid Game Genie letter".** The
   wrapper parses the whole rebuilt text and reports the line, which is
   the position in the list, not something the user typed.
   `applyCheats` strips the prefix before showing the toast.
Two races were designed out up front rather than hit:

- **Restore order.** Had the load assigned `emu` and then awaited the
  store reads, `requestAnimationFrame` would run between the awaits and
  a frame could execute before `set_battery`; Zelda writes SRAM during
  its first frames (the dirty flush in the table shows it does). The
  load restores into a local before publishing it, and the table
  records 0 frames run before the post-reload check.
- **Text fields and the controller map.** The Phase 2 keydown handler
  mapped `z`, `x`, Enter and `p` regardless of target, so typing a code
  would have pressed A, B and Start and paused the game. The `typing(e)`
  guard skips text inputs.
