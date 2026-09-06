// Headless browser check of the IndexedDB store (issue #51,
// docs/debugging/WASM_WEB_STORE.md). Not part of `npm test`: it needs
// Playwright with a Chromium build and two local servers:
//
//   python3 -m http.server 8791 --bind 127.0.0.1 --directory web      (the page)
//   a second server with Access-Control-Allow-Origin: * serving the ROMs
//
// then
//
//   NODE_PATH=<dir with node_modules/playwright> node test/browser-store.mjs \
//       http://127.0.0.1:8791/index.html http://127.0.0.1:8766
//
// ROMs are never copied into the repository; the script fetches
// zelda.nes and mario.nes from the second server inside the page and
// hands the bytes to window.nesLoadRom. Every check runs in one browser
// context so IndexedDB survives the page reloads.
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const PAGE = process.argv[2] ?? "http://127.0.0.1:8791/index.html";
const ROMS = process.argv[3] ?? "http://127.0.0.1:8766";
const results = [];
const record = (check, result) => {
  results.push([check, result]);
  console.log(`  ${check}: ${result}`);
};

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
const page = await context.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("console", (m) => {
  if (m.type() === "error" || m.type() === "warning") errors.push(`${m.type()}: ${m.text()}`);
});

async function open() {
  await page.goto(PAGE);
  await page.waitForFunction(() => window.nesStore && window.nesStats.core, null, { timeout: 15000 });
}

async function loadRom(name) {
  return page.evaluate(async ([base, name]) => {
    const r = await fetch(`${base}/${name}`);
    const bytes = new Uint8Array(await r.arrayBuffer());
    await window.nesLoadRom(bytes, name);
    window.nesApp.setPaused(true);
    return { crc: window.nesStats.crc, mapper: window.nesApp.emu.mapper_id(), seeded: window.nesStats.cheatsSeeded };
  }, [ROMS, name]);
}

const HASH = `(b)=>{let x=0x811c9dc5;for(let i=0;i<b.length;i++)x=Math.imul(x^b[i],0x01000193)>>>0;return x.toString(16).padStart(8,"0");}`;

try {
  await open();
  await page.evaluate(() => window.nesStore.clear());

  // ---- (a) battery RAM survives a page reload
  console.log("(a) battery");
  let info = await loadRom("zelda.nes");
  assert.equal(info.crc, "3FE272FB");
  assert.equal(info.mapper, 1);
  const before = await page.evaluate(async (hashSrc) => {
    const hash = eval(hashSrc);
    const emu = window.nesApp.emu;
    const ram = new Uint8Array(0x2000);
    for (let i = 0; i < ram.length; i++) ram[i] = (i * 31 + 7) & 0xff;
    if (!emu.set_battery(ram)) throw new Error("set_battery refused");
    const flushed = await window.nesApp.flushBattery(true);
    const stored = await window.nesStore.getBattery(window.nesStats.crc);
    return { flushed, live: hash(emu.battery()), stored: hash(stored), size: stored.length };
  }, HASH);
  assert.equal(before.flushed, true);
  assert.equal(before.live, before.stored);
  record("battery: set_battery + forced flush stores 8192 bytes", `hash ${before.stored}, ${before.size} bytes`);
  // The interval flush only fires on the dirty flag; count what the
  // game itself does over 6.5 s of play.
  const interval = await page.evaluate(async () => {
    const s = window.nesStats;
    window.nesApp.setPaused(false);
    const f0 = s.batteryFlushes;
    await new Promise((r) => setTimeout(r, 6500));
    window.nesApp.setPaused(true);
    return { flushes: s.batteryFlushes - f0, frames: s.frames, dirty: window.nesApp.emu.battery_dirty() };
  });
  record("battery: 5 s dirty-flag flush while Zelda runs", `${interval.flushes} flush(es) in ${interval.frames} frames, dirty now ${interval.dirty}`);
  const expected = await page.evaluate(async (hashSrc) => {
    const hash = eval(hashSrc);
    await window.nesApp.flushBattery(true);
    return hash(window.nesApp.emu.battery());
  }, HASH);
  await open();
  info = await loadRom("zelda.nes");
  const after = await page.evaluate((hashSrc) => ({ hash: eval(hashSrc)(window.nesApp.emu.battery()), frames: window.nesStats.frames }), HASH);
  assert.equal(after.hash, expected);
  record("battery: reload page, reload ROM, bytes restored", `hash ${after.hash} == ${expected}, ${after.frames} frames run before the check`);

  // ---- (b) state slot round trip
  console.log("(b) states");
  const st = await page.evaluate(async () => {
    const app = window.nesApp;
    app.runFrames(30);
    const saved = await app.saveState(1);
    app.runFrames(1);
    const h1 = app.frameHash();
    app.runFrames(120);
    const moved = app.frameHash();
    const loaded = await app.loadState(1);
    app.runFrames(1);
    const h2 = app.frameHash();
    const rec = await window.nesStore.getState(window.nesStats.crc, 1);
    const li = document.querySelector('#slots li[data-slot="1"]');
    const empty = await app.loadState(5);
    return { saved, loaded, h1, moved, h2, core: rec.core, size: rec.bytes.length, at: rec.at, li: li.textContent, active: li.classList.contains("active"), empty, toast: window.nesApp.osdText(), slot: window.nesStats.slot };
  });
  assert.equal(st.saved, true);
  assert.equal(st.loaded, true);
  assert.equal(st.h1, st.h2);
  assert.notEqual(st.h1, st.moved);
  assert.equal(st.empty, false);
  assert.match(st.toast, /empty/);
  record("state: save slot 1, run 120 frames, load, frame hash matches", `${st.h1} == ${st.h2} (moved ${st.moved}); ${st.size} bytes, core ${st.core}`);
  record("state: slot list shows time and core, empty slot 5 refuses", `"${st.li}" active=${st.active}; toast "${st.toast}"`);
  // Keys: F6 wraps 1 -> 9, F7 back to 1, F5 saves the current slot, F8 loads it.
  await page.evaluate(() => window.nesApp.setSlot(1));
  await page.focus("#screen-wrap");
  await page.keyboard.press("F6");
  const wrapped = await page.evaluate(() => window.nesStats.slot);
  assert.equal(wrapped, 9);
  await page.keyboard.press("F7");
  await page.keyboard.press("F7");
  await page.keyboard.press("F5");
  await page.waitForFunction(() => document.querySelector('#slots li[data-slot="2"]').textContent.includes("20"));
  await page.evaluate(() => window.nesApp.runFrames(60));
  await page.keyboard.press("F8");
  await page.waitForFunction(() => window.nesApp.osdText().startsWith("Loaded slot 2"));
  const keys = await page.evaluate(async () => ({ slot: window.nesStats.slot, slots: (await window.nesStore.listStates(window.nesStats.crc)).map((s) => s.slot) }));
  assert.deepEqual(keys.slots, [1, 2]);
  record("keys: F6 wraps to 9, F7 F7 to 2, F5 saves, F8 loads", `slot ${keys.slot}, used slots ${keys.slots}`);
  // Buttons.
  await page.click('#slots li[data-slot="3"]');
  await page.click("#btn-save");
  await page.waitForFunction(() => document.querySelector('#slots li[data-slot="3"]').textContent.includes("20"));
  await page.click("#btn-delete-state");
  await page.waitForFunction(() => document.querySelector('#slots li[data-slot="3"]').textContent.includes("empty"));
  record("buttons: click slot 3, Save, Delete", "slot 3 used then empty");

  // ---- (c) cheats
  console.log("(c) cheats");
  info = await loadRom("mario.nes");
  assert.equal(info.crc, "8E2BD25C");
  assert.equal(info.seeded, true);
  const seeded = await page.evaluate(() => {
    const list = window.nesApp.cheats();
    return { count: list.length, enabled: list.filter((c) => c.enabled).length, sxiopo: list.findIndex((c) => c.code === "SXIOPO"), note: document.getElementById("cheats-note").textContent, rows: document.querySelectorAll("#cheat-list li").length };
  });
  assert.ok(seeded.count > 0);
  assert.equal(seeded.enabled, 0);
  assert.ok(seeded.sxiopo >= 0);
  record("cheats: mario seeds from cheats.json, all off", `${seeded.count} codes, ${seeded.enabled} enabled, SXIOPO at ${seeded.sxiopo}, ${seeded.rows} rows`);
  const stored0 = await page.evaluate(() => window.nesStore.getCheats(window.nesStats.crc));
  assert.equal(stored0, null);
  // Enable the bundled SXIOPO through the checkbox, add a raw code via
  // the form, and try a bad code.
  await page.check(`#cheat-list li:nth-child(${seeded.sxiopo + 1}) input[type=checkbox]`);
  await page.waitForFunction(() => window.nesApp.emu.cheats_text().includes("SXIOPO\t1"));
  await page.fill("#cheat-code", "075a:02");
  await page.fill("#cheat-desc", "two coins");
  await page.click("#btn-cheat-add");
  await page.waitForFunction(() => window.nesApp.emu.cheats_text().includes("075A:02"));
  const badErr = await page.evaluate(() => window.nesApp.addCheat("NOTACODE!", "bad"));
  assert.ok(badErr && /letter|length|code/i.test(badErr), `error string: ${badErr}`);
  const dup = await page.evaluate(() => window.nesApp.addCheat("SXIOPO", "infinite lives"));
  assert.equal(dup, null);
  const c1 = await page.evaluate(async () => ({ text: window.nesApp.emu.cheats_text(), stored: await window.nesStore.getCheats(window.nesStats.crc), toast: window.nesApp.osdText(), codeBox: document.getElementById("cheat-code").value }));
  assert.match(c1.text, /SXIOPO\t1\t/);
  assert.match(c1.text, /075A:02\t1\ttwo coins/);
  assert.equal(c1.stored, c1.text);
  assert.equal(c1.codeBox, "");
  record("cheats: toggle SXIOPO on, add 075A:02 via form, bad code rejected", `error "${badErr}"; text stored (${c1.text.length} chars)`);
  await open();
  info = await loadRom("mario.nes");
  assert.equal(info.seeded, false);
  const c2 = await page.evaluate(() => window.nesApp.emu.cheats_text());
  assert.equal(c2, c1.text);
  record("cheats: survive page reload", `cheats_text identical, seeded=${info.seeded}`);
  const del = await page.evaluate(async () => {
    const i = window.nesApp.cheats().findIndex((c) => c.code === "075A:02");
    const err = await window.nesApp.deleteCheat(i);
    return { err, text: window.nesApp.emu.cheats_text() };
  });
  assert.equal(del.err, null);
  assert.doesNotMatch(del.text, /075A:02/);
  record("cheats: delete", "075A:02 removed");

  // ---- (d) export and import
  console.log("(d) export/import");
  const dumpBefore = await page.evaluate(async (hashSrc) => {
    const hash = eval(hashSrc);
    const d = await window.nesStore.dump();
    return JSON.stringify({ roms: d.roms.map((r) => r.crc).sort(), battery: d.battery.map((b) => [b.crc, hash(b.bytes)]), states: d.states.map((s) => [s.key, hash(s.bytes), s.core]), cheats: d.cheats.map((c) => [c.crc, c.text]) });
  }, HASH);
  const json = await page.evaluate(async () => JSON.stringify(await window.nesApp.exportJson()));
  const doc = JSON.parse(json);
  assert.equal(doc.format, "nes-emu-web-store");
  assert.deepEqual(Object.keys(doc.roms).sort(), ["3FE272FB", "8E2BD25C"]);
  const counts = await page.evaluate(async (json) => {
    await window.nesStore.clear();
    const empty = await window.nesStore.dump();
    const counts = await window.nesApp.importStore(json);
    return { emptyStates: empty.states.length, counts };
  }, json);
  assert.equal(counts.emptyStates, 0);
  const dumpAfter = await page.evaluate(async (hashSrc) => {
    const hash = eval(hashSrc);
    const d = await window.nesStore.dump();
    return JSON.stringify({ roms: d.roms.map((r) => r.crc).sort(), battery: d.battery.map((b) => [b.crc, hash(b.bytes)]), states: d.states.map((s) => [s.key, hash(s.bytes), s.core]), cheats: d.cheats.map((c) => [c.crc, c.text]) });
  }, HASH);
  assert.equal(dumpAfter, dumpBefore);
  record("export/import: clear store, import the export, dump identical", `${json.length} chars; imported ${JSON.stringify(counts.counts)}`);
  const badImport = await page.evaluate(() => window.nesApp.importStore('{"format":"other"}'));
  assert.equal(badImport, null);
  record("import: foreign file rejected", "returns null with a toast");
  // The Export button must produce a download.
  const [download] = await Promise.all([page.waitForEvent("download"), page.click("#btn-export")]);
  record("export button: download event", download.suggestedFilename());

  console.log("\nall store checks passed");
  const noise = errors.filter((e) => !/AudioContext|favicon/.test(e));
  if (noise.length) console.log("console errors/warnings:", noise);
} finally {
  await browser.close();
}
