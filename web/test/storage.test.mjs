// Unit tests for the pure parts of storage.js: base64 and the export
// and import encoding (issue #51). IndexedDB itself is covered by the
// headless browser pass in docs/debugging/WASM_WEB_STORE.md.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  base64ToBytes,
  bytesToBase64,
  crcKey,
  decodeExport,
  encodeExport,
  EXPORT_FORMAT,
  EXPORT_VERSION,
  stateKey,
} from "../storage.js";

test("crcKey is eight upper-case hex digits", () => {
  assert.equal(crcKey(0x8e2bd25c), "8E2BD25C");
  assert.equal(crcKey(0x1), "00000001");
  assert.equal(crcKey(0xffffffff), "FFFFFFFF");
  assert.equal(stateKey("8E2BD25C", 3), "8E2BD25C:3");
});

test("base64 round trips arbitrary bytes, including large buffers", () => {
  const small = new Uint8Array([0, 1, 2, 250, 255]);
  assert.equal(bytesToBase64(small), "AAEC+v8=");
  assert.deepEqual(base64ToBytes("AAEC+v8="), small);
  const big = new Uint8Array(200_000);
  for (let i = 0; i < big.length; i++) big[i] = (i * 7 + 3) & 0xff;
  assert.deepEqual(base64ToBytes(bytesToBase64(big)), big);
  assert.deepEqual(base64ToBytes(bytesToBase64(new Uint8Array(0))), new Uint8Array(0));
});

function sample() {
  return {
    roms: [{ crc: "8E2BD25C", name: "mario.nes", seenAt: 5 }],
    battery: [{ crc: "3FE272FB", bytes: new Uint8Array([1, 2, 3]), at: 10 }],
    states: [
      { key: "8E2BD25C:1", crc: "8E2BD25C", slot: 1, bytes: new Uint8Array([78, 69, 83, 83]), at: 20, core: "0.13.0" },
      { key: "8E2BD25C:9", crc: "8E2BD25C", slot: 9, bytes: new Uint8Array([9]), at: 21, core: "0.13.0" },
    ],
    cheats: [{ crc: "8E2BD25C", text: "SXIOPO\t1\tInfinite lives\n", at: 30 }],
  };
}

test("encodeExport produces the documented shape", () => {
  const doc = encodeExport(sample(), 1234);
  assert.equal(doc.format, EXPORT_FORMAT);
  assert.equal(doc.version, EXPORT_VERSION);
  assert.equal(doc.exportedAt, 1234);
  assert.deepEqual(Object.keys(doc.roms).sort(), ["3FE272FB", "8E2BD25C"]);
  assert.equal(doc.roms["8E2BD25C"].name, "mario.nes");
  assert.equal(doc.roms["8E2BD25C"].states["1"].bytes, "TkVTUw==");
  assert.equal(doc.roms["8E2BD25C"].states["1"].core, "0.13.0");
  assert.equal(doc.roms["8E2BD25C"].cheats.text, "SXIOPO\t1\tInfinite lives\n");
  assert.equal(doc.roms["8E2BD25C"].battery, null);
  assert.equal(doc.roms["3FE272FB"].battery.bytes, "AQID");
  assert.equal(doc.roms["3FE272FB"].name, null);
  // JSON-safe: no typed arrays leak through.
  assert.equal(JSON.parse(JSON.stringify(doc)).roms["3FE272FB"].battery.bytes, "AQID");
});

test("decodeExport inverts encodeExport through JSON text", () => {
  const doc = JSON.parse(JSON.stringify(encodeExport(sample(), 1)));
  const back = decodeExport(doc);
  assert.deepEqual(
    back.roms.map((r) => r.crc).sort(),
    ["3FE272FB", "8E2BD25C"]
  );
  assert.deepEqual(back.battery, [{ crc: "3FE272FB", bytes: new Uint8Array([1, 2, 3]), at: 10 }]);
  assert.deepEqual(back.cheats, sample().cheats);
  assert.deepEqual(back.states, sample().states);
});

test("decodeExport rejects foreign files", () => {
  assert.throws(() => decodeExport(null), /not a nes-emu/);
  assert.throws(() => decodeExport({ format: "other" }), /not a nes-emu/);
  assert.throws(() => decodeExport({ format: EXPORT_FORMAT, version: 99 }), /version/);
  assert.throws(() => decodeExport({ format: EXPORT_FORMAT, version: EXPORT_VERSION, roms: { zz: {} } }), /CRC/);
  assert.throws(
    () => decodeExport({ format: EXPORT_FORMAT, version: EXPORT_VERSION, roms: { "00000001": { states: { 10: { bytes: "" } } } } }),
    /slot/
  );
  assert.deepEqual(decodeExport({ format: EXPORT_FORMAT, version: EXPORT_VERSION }), {
    roms: [],
    battery: [],
    states: [],
    cheats: [],
  });
});
