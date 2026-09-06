#!/usr/bin/env node
// Size budget for the wasm module (issue #53, docs/debugging/WASM_WEB_DEPLOY.md).
//
// Usage: node scripts/check-size.mjs [path/to/module.wasm ...]
// With no arguments it checks whichever of pkg/ and pkg-node/ exist (the
// web and Node builds of wasm-pack) and fails if neither is present.
//
// Fails (exit 1) when a module is larger than BUDGET_BYTES or carries
// debug information: a "name" custom section or any ".debug_*" section.
// wasm-pack runs wasm-opt and strips those already; this makes sure a
// profile change does not bring them back unnoticed. Sizes are printed in
// bytes and KB (1 KB = 1024 bytes).

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const BUDGET_BYTES = 500 * 1024;
const DEFAULTS = ["pkg/nes_emu_web_bg.wasm", "pkg-node/nes_emu_web_bg.wasm"];

// Names of the custom sections in a wasm binary (section id 0).
function customSections(bytes) {
  if (bytes.length < 8 || bytes.readUInt32LE(0) !== 0x6d736100) {
    throw new Error("not a wasm module (bad magic)");
  }
  let p = 8;
  const leb = () => {
    let r = 0;
    let s = 0;
    for (;;) {
      const x = bytes[p++];
      r |= (x & 0x7f) << s;
      if (!(x & 0x80)) break;
      s += 7;
    }
    return r >>> 0;
  };
  const names = [];
  while (p < bytes.length) {
    const id = bytes[p++];
    const size = leb();
    const end = p + size;
    if (id === 0) {
      const n = leb();
      names.push(bytes.subarray(p, p + n).toString("utf8"));
    }
    p = end;
  }
  return names;
}

const args = process.argv.slice(2);
const paths = args.length ? args : DEFAULTS.filter((p) => existsSync(p));
if (paths.length === 0) {
  console.error(`check-size: no module found; run npm run build (looked for ${DEFAULTS.join(", ")})`);
  process.exit(1);
}

let failed = false;
for (const path of paths) {
  const bytes = readFileSync(resolve(path));
  const kb = (bytes.length / 1024).toFixed(1);
  const budgetKb = (BUDGET_BYTES / 1024).toFixed(0);
  const sections = customSections(bytes);
  const debug = sections.filter((n) => n === "name" || n.startsWith(".debug_"));
  console.log(`${path}: ${bytes.length} bytes (${kb} KB), budget ${BUDGET_BYTES} bytes (${budgetKb} KB)`);
  console.log(`${path}: custom sections: ${sections.join(", ") || "none"}`);
  if (bytes.length > BUDGET_BYTES) {
    console.error(`check-size: ${path} exceeds the ${budgetKb} KB budget`);
    failed = true;
  }
  if (debug.length) {
    console.error(`check-size: ${path} carries debug info (${debug.join(", ")})`);
    failed = true;
  }
}
process.exit(failed ? 1 : 0);
