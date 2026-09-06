#!/usr/bin/env node
// Convert the bundled cheat database (../cheats/*.cht) into one JSON file
// the page fetches (issue #51, docs/debugging/WASM_WEB_STORE.md).
//
// Output shape, keyed by upper-case 8 digit CRC-32 of the ROM image after
// the iNES header, as `# crc32:` header lines declare it (the same rule
// as `nes_emu::cheat::database_crcs`). A file with several `# crc32:`
// lines appears under every one of them:
//
//   { "8E2BD25C": { "name": "Super Mario Bros. (World)", "text": "..." } }
//
// `text` is the .cht file body unchanged, so the page can hand it straight
// to Emulator.set_cheats_text.
import { readdirSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** Every `# crc32: XXXXXXXX` value in a .cht file, normalised to eight
 *  upper-case hex digits. Invalid values are skipped. */
export function databaseCrcs(text) {
  const out = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("#")) continue;
    const rest = line.slice(1).trim();
    if (!rest.startsWith("crc32:")) continue;
    const value = rest.slice("crc32:".length).trim();
    if (!/^[0-9a-fA-F]{1,8}$/.test(value)) continue;
    out.push(value.toUpperCase().padStart(8, "0"));
  }
  return out;
}

/** The game name: the first `# ` comment line, or the file name. */
export function databaseName(text, fallback) {
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    if (!line.startsWith("#")) break;
    const rest = line.slice(1).trim();
    if (rest && !rest.includes(":")) return rest;
  }
  return fallback;
}

/** Build the JSON object from `[{ name, text }]` entries (name is the file
 *  name, used as the fallback game name). Files are processed in name
 *  order and the first file to claim a CRC wins, matching
 *  `find_in_database`. */
export function buildCheatsJson(files) {
  const sorted = [...files].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  const out = {};
  for (const file of sorted) {
    const fallback = file.name.replace(/\.cht$/i, "");
    const entry = { name: databaseName(file.text, fallback), text: file.text };
    for (const crc of databaseCrcs(file.text)) {
      if (!(crc in out)) out[crc] = entry;
    }
  }
  return out;
}

export function readCheatDir(dir) {
  return readdirSync(dir)
    .filter((name) => name.toLowerCase().endsWith(".cht"))
    .map((name) => ({ name, text: readFileSync(join(dir, name), "utf8") }));
}

function main() {
  const here = dirname(fileURLToPath(import.meta.url));
  const src = resolve(process.argv[2] ?? join(here, "..", "..", "cheats"));
  const dst = resolve(process.argv[3] ?? join(here, "..", "cheats.json"));
  const files = readCheatDir(src);
  const json = buildCheatsJson(files);
  mkdirSync(dirname(dst), { recursive: true });
  writeFileSync(dst, JSON.stringify(json, null, 1) + "\n");
  console.log(`cheats.json: ${files.length} files, ${Object.keys(json).length} CRCs -> ${dst}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main();
}
