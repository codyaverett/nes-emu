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

emu.free();
console.log(
  `smoke ok: 60 frames in ${elapsed.toFixed(1)} ms ` +
    `(${(60000 / elapsed).toFixed(0)} fps), ${audioTotal} audio samples, ` +
    `core ${core_version()}`
);
