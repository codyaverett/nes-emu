// Headless smoke test for the wasm build (issue #49). Builds a synthetic
// NROM image (header plus NOPs, reset vector at $8000), runs 60 frames
// through the Node target of the wasm package and checks the video and
// audio outputs. Run `wasm-pack build --release --target nodejs
// --out-dir pkg-node` first, or use `npm test` in this directory.
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const wasm = require("../pkg-node/nes_emu_web.js");
const { Emulator, Button, core_version } = wasm;

function syntheticRom(battery) {
  const rom = new Uint8Array(16 + 0x8000);
  rom.set([0x4e, 0x45, 0x53, 0x1a, 2, 0, battery ? 0x02 : 0x00, 0]);
  rom.fill(0xea, 16);
  rom[16 + 0x7ffc] = 0x00;
  rom[16 + 0x7ffd] = 0x80;
  return rom;
}

assert.throws(() => new Emulator(new Uint8Array([1, 2, 3])), /ROM/);

const emu = new Emulator(syntheticRom(true));
assert.equal(emu.mapper_id(), 0);
assert.equal(emu.battery_backed(), true);
assert.equal(emu.frame_width(), 256);
assert.equal(emu.frame_height(), 240);
assert.equal(emu.audio_sample_rate(), 44100);

emu.set_button(0, Button.Start, true);
let audioTotal = 0;
const started = performance.now();
for (let i = 0; i < 60; i++) {
  emu.run_frame();
  audioTotal += emu.take_audio().length;
}
const elapsed = performance.now() - started;
emu.set_button(0, Button.Start, false);

const frame = emu.frame_rgba();
assert.ok(frame instanceof Uint8ClampedArray, "frame is a Uint8ClampedArray");
assert.equal(frame.length, 256 * 240 * 4);
for (let i = 3; i < frame.length; i += 4) assert.equal(frame[i], 255);
// 60 frames of 44.1 kHz audio, within one frame's worth of samples.
assert.ok(Math.abs(audioTotal - 44100) < 800, `audio samples ${audioTotal}`);

const state = emu.save_state();
assert.equal(String.fromCharCode(...state.subarray(0, 4)), "NESS");
emu.run_frame();
emu.load_state(state);
assert.throws(() => emu.load_state(new Uint8Array([1, 2, 3])));

const ram = new Uint8Array(0x2000).fill(0x5a);
assert.equal(emu.set_battery(ram), true);
assert.equal(emu.battery_dirty(), false);
assert.equal(emu.battery()[0x123], 0x5a);

emu.set_cheats_text("SXIOPO\t1\tinfinite lives\n");
assert.match(emu.cheats_text(), /SXIOPO/);
assert.throws(() => emu.set_cheats_text("NOTACODE!\t1\tx\n"));
assert.notEqual(emu.rom_crc32(), 0);
assert.match(core_version(), /^\d+\.\d+\.\d+$/);

// Shared overlay UI (issue #52, docs/plans/SHARED_OVERLAY_UI.md).
emu.set_now_ms(Date.now());
assert.equal(emu.ui_active(), false);
assert.equal(emu.key_down("KeyZ"), false, "controller keys are the page's");
assert.equal(emu.key_down("Backquote"), true, "backquote opens the palette");
assert.equal(emu.ui_active(), true);
assert.deepEqual(Array.from(emu.overlay_size()), [720, 672]);
assert.equal(emu.overlay_visible(), true);
const overlay = emu.overlay_rgba();
assert.ok(overlay instanceof Uint8ClampedArray, "overlay is a Uint8ClampedArray");
assert.equal(overlay.length, 720 * 672 * 4);
let opaque = 0;
for (let i = 3; i < overlay.length; i += 4) if (overlay[i] > 0) opaque++;
assert.ok(opaque > 1000, `overlay has alpha (${opaque} pixels)`);
assert.equal(emu.key_down("KeyP"), true, "typing into the palette is consumed");
assert.equal(emu.paused(), false);
assert.equal(emu.key_down("Escape"), true);
assert.equal(emu.ui_active(), false);
assert.equal(emu.key_down("F1"), true, "F1 opens Help");
assert.equal(emu.key_down("Escape"), true);

// Rewind: snapshots recorded by run_frame, stepped back while Backspace
// is held.
const seconds = emu.rewind_seconds();
assert.ok(seconds >= 0.5, `rewind buffer holds ${seconds} s`);
assert.equal(emu.key_down("Backspace"), true);
assert.equal(emu.rewinding(), true);
emu.rewind_step();
emu.rewind_step();
assert.ok(emu.rewind_seconds() < seconds, "rewind_step consumes snapshots");
emu.key_up("Backspace");
assert.equal(emu.rewinding(), false);

// F5 saves through the host and marks slot 1 dirty for the page.
assert.deepEqual(Array.from(emu.take_dirty_slots()), []);
assert.equal(emu.key_down("F5"), true);
assert.deepEqual(Array.from(emu.take_dirty_slots()), [1]);
assert.deepEqual(Array.from(emu.take_dirty_slots()), []);
const slot1 = emu.slot_bytes(1);
assert.equal(String.fromCharCode(...slot1.subarray(0, 4)), "NESS");
assert.equal(emu.slot_bytes(2), undefined);
assert.equal(emu.key_down("F7"), true);
assert.equal(emu.slot(), 2);
emu.set_slot_cache(2, slot1, Date.now());
assert.deepEqual(Array.from(emu.take_dirty_slots()), []);
assert.equal(emu.key_down("F8"), true);
assert.equal(emu.overlay_visible(), true, "the Loaded slot toast shows");
emu.osd_message("hello from the page");
assert.equal(emu.osd_text(), "hello from the page");
assert.equal(emu.take_cheats_dirty(), false);

emu.free();
console.log(
  `smoke ok: 60 frames in ${elapsed.toFixed(1)} ms ` +
    `(${(60000 / elapsed).toFixed(0)} fps), ${audioTotal} audio samples, ` +
    `core ${core_version()}`
);
