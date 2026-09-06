// Browser front end for nes-emu (issue #50, docs/plans/WASM_WEB.md).
//
// The core runs in wasm (web/src/lib.rs). This file owns the canvas, the
// audio pipeline, the controller mapping and pacing. Audio is the master
// clock, as in src/main.rs: each animation frame emulates until roughly
// AUDIO_TARGET samples are queued ahead of the AudioWorklet, at most
// MAX_FRAMES_PER_TICK frames, so the display refresh rate never changes
// the game speed. Stats are exposed on window.nesStats for headless
// checks (see docs/debugging/WASM_WEB_PAGE.md). Battery RAM, state
// slots and cheats persist in IndexedDB through storage.js, keyed by
// ROM CRC-32 (docs/debugging/WASM_WEB_STORE.md).
//
// The command palette, the tool pages, the hotkeys (R, P, N, M, +/-,
// F1, F5-F8, Backspace rewind), toasts and rewind live in the core's
// shared UI (docs/plans/SHARED_OVERLAY_UI.md): every key goes to
// emu.key_down before the controller table, the overlay canvas shows
// emu.overlay_rgba, and the page mirrors pause, mute, volume, crop and
// slot from the core into its controls once per tick. Slot and cheat
// writes made by the UI reach IndexedDB through the dirty flags.
import init, { Emulator, Button, core_version } from "./pkg/nes_emu_web.js";
import { openStore, crcKey, SLOTS } from "./storage.js";

const BATTERY_FLUSH_MS = 5000;
const CHEATS_URL = "./cheats.json";

const SAMPLE_RATE = 44100;
// Samples kept queued ahead of the worklet: about 3.3 frames, 55 ms
// (AUDIO_TARGET_SAMPLES in src/main.rs).
const AUDIO_TARGET = 2450;
const MAX_FRAMES_PER_TICK = 4;
const OVERSCAN = 8;
const FRAME_W = 256;
const FRAME_H = 240;

const KEYS = {
  KeyZ: [0, Button.A],
  KeyX: [0, Button.B],
  ShiftRight: [0, Button.Select],
  Enter: [0, Button.Start],
  ArrowUp: [0, Button.Up],
  ArrowDown: [0, Button.Down],
  ArrowLeft: [0, Button.Left],
  ArrowRight: [0, Button.Right],
  Quote: [1, Button.A],
  Semicolon: [1, Button.B],
  Comma: [1, Button.Select],
  Period: [1, Button.Start],
  KeyI: [1, Button.Up],
  KeyK: [1, Button.Down],
  KeyJ: [1, Button.Left],
  KeyL: [1, Button.Right],
};

const $ = (id) => document.getElementById(id);
const canvas = $("screen");
const ctx2d = canvas.getContext("2d");
const overlay = $("overlay");
const overlay2d = overlay.getContext("2d");
const wrap = $("screen-wrap");
const gate = $("gate");
const statsEl = $("stats");

const stats = {
  core: null,
  rom: null,
  frames: 0,
  ticks: 0,
  fps: 0,
  emuFps: 0,
  queued: 0,
  underruns: 0,
  dropped: 0,
  audio: "off",
  paused: false,
  muted: false,
  volume: 1,
  crop: true,
  error: null,
  crc: null, // eight hex digits, the store key
  slot: 1,
  batteryFlushes: 0,
  cheatsSeeded: false,
  rewinding: false,
  uiActive: false,
  overlayVisible: false,
  overlayDraws: 0,
  slotWrites: 0,
  cheatWrites: 0,
};
window.nesStats = stats;

let emu = null;
let store = null; // storage.js API, opened at boot (null if IndexedDB failed)
let image = new ImageData(FRAME_W, FRAME_H);
let overlayImage = null; // ImageData sized to emu.overlay_size()
let overlayShown = false; // the overlay canvas has content to clear

// ---------------------------------------------------------------- audio

const audio = {
  ctx: null,
  node: null,
  gain: null,
  sent: 0, // samples posted to the worklet
  consumed: 0, // worklet's last reported consumed count
  reportTime: 0, // ctx.currentTime when that report arrived
  lastReport: null,
  reports: 0,
  ready: false,
};
window.nesAudio = audio;

async function startAudio() {
  if (audio.ready || audio.ctx) return;
  if (typeof AudioWorkletNode === "undefined") {
    stats.audio = "unsupported";
    console.warn("[nes] AudioWorklet unavailable (insecure context?); running silent");
    return;
  }
  try {
    const ctx = new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: "interactive" });
    audio.ctx = ctx;
    await ctx.audioWorklet.addModule("./audio-worklet.js");
    const node = new AudioWorkletNode(ctx, "nes-audio", { outputChannelCount: [1] });
    const gain = ctx.createGain();
    node.connect(gain).connect(ctx.destination);
    node.port.onmessage = (e) => {
      audio.consumed = e.data.consumed;
      audio.reportTime = ctx.currentTime;
      audio.lastReport = e.data;
      audio.reports++;
      stats.underruns = e.data.underruns;
      stats.dropped = e.data.dropped;
    };
    audio.node = node;
    audio.gain = gain;
    if (ctx.sampleRate !== SAMPLE_RATE) {
      console.warn(`[nes] AudioContext runs at ${ctx.sampleRate} Hz, wanted ${SAMPLE_RATE}`);
    }
    ctx.onstatechange = () => (stats.audio = ctx.state);
    await ctx.resume();
    audio.ready = true;
    stats.audio = ctx.state;
    console.log(`[nes] audio ${ctx.state} at ${ctx.sampleRate} Hz, base latency ${(ctx.baseLatency * 1000).toFixed(1)} ms`);
  } catch (err) {
    stats.audio = "failed";
    console.warn("[nes] audio setup failed, running silent:", err);
  }
}

/** Samples still queued ahead of the worklet, estimated from its last
 *  report plus the time elapsed since. */
function queuedAudio() {
  if (!audio.ready) return 0;
  const elapsed = Math.max(0, audio.ctx.currentTime - audio.reportTime);
  const consumed = audio.consumed + elapsed * audio.ctx.sampleRate;
  return Math.max(0, audio.sent - consumed);
}

function pushAudio() {
  const samples = emu.take_audio();
  if (!audio.ready || samples.length === 0) return;
  // Read the length first: the transfer detaches the buffer and leaves
  // samples.length at 0.
  const count = samples.length;
  audio.node.port.postMessage(samples, [samples.buffer]);
  audio.sent += count;
}

function clearAudio() {
  if (!audio.ready) return;
  audio.node.port.postMessage({ type: "clear" });
  audio.sent = audio.consumed;
  audio.reportTime = audio.ctx.currentTime;
}

// ---------------------------------------------------------------- video

function present() {
  image.data.set(emu.frame_rgba());
  const off = stats.crop ? -OVERSCAN : 0;
  ctx2d.putImageData(image, off, off);
}

/** Size the canvases for the crop state; the core is the source of
 *  truth (emu.crop_enabled), this only mirrors it into the DOM. */
function applyCrop(crop) {
  stats.crop = crop;
  canvas.width = crop ? FRAME_W - 2 * OVERSCAN : FRAME_W;
  canvas.height = crop ? FRAME_H - 2 * OVERSCAN : FRAME_H;
  wrap.classList.toggle("full-frame", !crop);
  $("btn-crop").textContent = crop ? "Full frame" : "Crop overscan";
  if (emu) {
    const [w, h] = emu.overlay_size();
    if (overlay.width !== w || overlay.height !== h) {
      overlay.width = w;
      overlay.height = h;
      overlayImage = new ImageData(w, h);
    }
    present();
    presentOverlay();
  }
}

function setCrop(crop) {
  if (emu) emu.set_crop(crop);
  else applyCrop(crop);
}

/** Draw the shared overlay (palette, pages, toasts, the volume bar)
 *  when the core has something to show, else clear it once. */
function presentOverlay() {
  if (!emu) return;
  const visible = emu.overlay_visible();
  stats.overlayVisible = visible;
  if (visible) {
    if (!overlayImage || overlayImage.width !== overlay.width || overlayImage.height !== overlay.height) {
      overlayImage = new ImageData(overlay.width, overlay.height);
    }
    overlayImage.data.set(emu.overlay_rgba());
    overlay2d.putImageData(overlayImage, 0, 0);
    overlayShown = true;
    stats.overlayDraws++;
  } else if (overlayShown) {
    overlay2d.clearRect(0, 0, overlay.width, overlay.height);
    overlayShown = false;
  }
}

/** A toast through the shared overlay; before a ROM is loaded there is
 *  no overlay, so it goes to the console. `ms` is kept for callers; the
 *  core's toast duration applies. */
function toast(text, ms = 1500) {
  void ms;
  if (emu) emu.osd_message(text);
  else console.log(`[nes] ${text}`);
}

/** Mirror the core's run-control state into the page's controls. */
function syncControls() {
  const paused = emu.paused();
  if (paused !== stats.paused) {
    stats.paused = paused;
    $("btn-pause").textContent = paused ? "Resume" : "Pause";
    clearAudio();
  }
  const muted = emu.muted();
  const volume = emu.volume();
  if (muted !== stats.muted || volume !== stats.volume) {
    stats.muted = muted;
    stats.volume = volume;
    if (audio.gain) audio.gain.gain.value = muted ? 0 : volume;
    $("btn-mute").textContent = muted ? "Unmute" : "Mute";
  }
  const crop = emu.crop_enabled();
  if (crop !== stats.crop) applyCrop(crop);
  const slot = emu.slot();
  if (slot !== stats.slot) {
    stats.slot = slot;
    for (const li of slotsEl.children) li.classList.toggle("active", Number(li.dataset.slot) === slot);
  }
  stats.uiActive = emu.ui_active();
  const rewinding = emu.rewinding();
  if (rewinding && !stats.rewinding) clearAudio();
  stats.rewinding = rewinding;
}

/** Persist what the shared UI changed: dirty slots and the cheat text. */
function persistDirty() {
  if (!emu) return;
  const slots = emu.take_dirty_slots();
  for (const slot of slots) {
    const bytes = emu.slot_bytes(slot);
    stats.slotWrites++;
    if (store && stats.crc && bytes) {
      store.setState(stats.crc, slot, bytes, stats.core).then(refreshSlots, (err) => {
        console.error("[nes] state store failed:", err);
        toast(`Store failed: ${err}`, 4000);
      });
    }
  }
  if (emu.take_cheats_dirty()) {
    stats.cheatWrites++;
    stats.cheatsSeeded = false;
    if (store && stats.crc) {
      store.setCheats(stats.crc, emu.cheats_text()).catch((err) => {
        console.error("[nes] cheat save failed:", err);
        toast(`Cheat save failed: ${err}`, 4000);
      });
    }
    renderCheats();
  }
}

// ---------------------------------------------------------------- loop

let lastStatsTime = performance.now();
let framesAtLastStats = 0;
let ticksAtLastStats = 0;

function tick(now) {
  requestAnimationFrame(tick);
  if (!emu) return;
  stats.ticks++;
  emu.set_now_ms(Date.now());
  emu.tick();
  syncControls();
  if (stats.rewinding) {
    // Backspace held: one snapshot back per display frame, no emulation
    // (App::rewind_frame drops the core's audio queue; ours was cleared
    // when the rewind started).
    emu.rewind_step();
    emu.take_audio();
    present();
  } else if (stats.paused) {
    if (emu.take_frame_advance()) {
      emu.run_frame();
      emu.take_audio();
      stats.frames++;
      present();
    }
  } else {
    let ran = 0;
    if (audio.ready && audio.ctx.state === "running") {
      while (queuedAudio() < AUDIO_TARGET && ran < MAX_FRAMES_PER_TICK) {
        emu.run_frame();
        pushAudio();
        ran++;
      }
    } else {
      emu.run_frame();
      emu.take_audio();
      ran = 1;
    }
    stats.frames += ran;
    if (ran > 0) present();
  }
  presentOverlay();
  persistDirty();
  stats.queued = Math.round(queuedAudio());
  if (now - lastStatsTime >= 1000) {
    const dt = (now - lastStatsTime) / 1000;
    stats.emuFps = (stats.frames - framesAtLastStats) / dt;
    stats.fps = (stats.ticks - ticksAtLastStats) / dt;
    lastStatsTime = now;
    framesAtLastStats = stats.frames;
    ticksAtLastStats = stats.ticks;
    statsEl.textContent =
      `${stats.rom}  mapper ${emu.mapper_id()}  crc ${emu.rom_crc32().toString(16).padStart(8, "0")}  ` +
      `emu ${stats.emuFps.toFixed(1)} fps  display ${stats.fps.toFixed(0)} Hz  ` +
      `audio ${stats.audio} queued ${stats.queued} underruns ${stats.underruns}`;
  }
}
requestAnimationFrame(tick);

setInterval(() => {
  if (emu) {
    console.log(
      `[nes] frames=${stats.frames} emu=${stats.emuFps.toFixed(1)}fps display=${stats.fps.toFixed(0)}Hz ` +
        `queued=${stats.queued} underruns=${stats.underruns} dropped=${stats.dropped} audio=${stats.audio}`
    );
  }
}, 5000);

// ---------------------------------------------------------------- ROMs

async function loadRomFile(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  await startAudio();
  // Build into a local first: the loop emulates as soon as `emu` is set,
  // and battery RAM and cheats must be restored before the first frame.
  let next;
  try {
    next = new Emulator(bytes);
  } catch (err) {
    stats.error = String(err);
    toast(`Cannot load ${file.name}: ${err}`, 4000);
    console.error("[nes] load failed:", err);
    return;
  }
  next.set_now_ms(Date.now());
  await flushBattery(false);
  if (emu) emu.free();
  emu = null;
  const crc = crcKey(next.rom_crc32());
  await restoreFromStore(next, crc, file.name);
  emu = next;
  stats.error = null;
  stats.rom = file.name;
  stats.crc = crc;
  stats.frames = 0;
  stats.paused = false;
  stats.slot = 1;
  stats.rewinding = false;
  overlayShown = true; // force one clear of whatever the last ROM drew
  $("btn-pause").textContent = "Pause";
  applyCrop(emu.crop_enabled());
  if (audio.gain) audio.gain.gain.value = emu.muted() ? 0 : emu.volume();
  clearAudio();
  gate.hidden = true;
  document.title = `${file.name} - nes-emu`;
  console.log(`[nes] loaded ${file.name}: ${bytes.length} bytes, mapper ${emu.mapper_id()}, crc32 ${crc}`);
  toast(`Loaded ${file.name}`);
  await refreshSlots();
  renderCheats();
  wrap.focus();
}

// ---------------------------------------------------------------- store

/** Restore battery RAM and cheats for `crc` into `target` (not yet the
 *  live emulator). Cheats come from the store, else from the bundled
 *  database, else stay empty. */
async function restoreFromStore(target, crc, name) {
  stats.cheatsSeeded = false;
  if (!store) return;
  try {
    await store.touchRom(crc, name);
    if (target.battery_backed()) {
      const saved = await store.getBattery(crc);
      if (saved) {
        if (target.set_battery(saved)) console.log(`[nes] battery restored: ${saved.length} bytes`);
        else console.warn(`[nes] stored battery RAM (${saved.length} bytes) does not fit this board`);
      }
    }
    // The shared UI's States page and F8 read the slot cache; fill it
    // from the store so the first frames already know the slots.
    for (const s of await store.listStates(crc)) {
      const rec = await store.getState(crc, s.slot);
      if (rec) target.set_slot_cache(s.slot, rec.bytes, rec.at ?? 0);
    }
    let text = await store.getCheats(crc);
    if (text === null) {
      text = await bundledCheats(crc);
      if (text !== null) stats.cheatsSeeded = true;
    }
    if (text !== null) {
      try {
        target.set_cheats_text(text);
      } catch (err) {
        console.warn("[nes] stored cheats rejected:", err);
        toast(`Stored cheats rejected: ${err}`, 4000);
      }
    }
  } catch (err) {
    console.error("[nes] store restore failed:", err);
    toast(`Store restore failed: ${err}`, 4000);
  }
}

let cheatsDb = null; // cheats.json once fetched, false when unavailable

/** The bundled cheat text for `crc` with every code forced off, or
 *  null. Not written to the store: a user edit persists it, so an
 *  unedited game keeps following database updates. */
async function bundledCheats(crc) {
  if (cheatsDb === null) {
    try {
      const res = await fetch(CHEATS_URL, { cache: "no-cache" });
      cheatsDb = res.ok ? await res.json() : false;
      if (!res.ok) console.warn(`[nes] ${CHEATS_URL} ${res.status}; run npm run build to generate it`);
    } catch (err) {
      cheatsDb = false;
      console.warn("[nes] cheats.json unavailable:", err);
    }
  }
  const entry = cheatsDb && cheatsDb[crc];
  if (!entry) return null;
  console.log(`[nes] bundled cheats: ${entry.name}`);
  return parseCheatLines(entry.text)
    .map((c) => cheatLine({ ...c, enabled: false }))
    .join("");
}

/** Write battery RAM to the store when it changed (or always with
 *  `force`). `sync` issues the put without awaiting anything first, for
 *  pagehide. Returns true when a write was issued. */
function flushBattery(force = false, sync = false) {
  if (!emu || !store || !stats.crc || !emu.battery_backed()) return Promise.resolve(false);
  if (!force && !emu.battery_dirty()) return Promise.resolve(false);
  const bytes = emu.battery();
  if (!bytes) return Promise.resolve(false);
  emu.mark_battery_saved();
  stats.batteryFlushes++;
  const p = sync ? store.setBatterySync(stats.crc, bytes) : store.setBattery(stats.crc, bytes);
  return p.then(
    () => true,
    (err) => {
      console.error("[nes] battery flush failed:", err);
      return false;
    }
  );
}

setInterval(() => flushBattery(false), BATTERY_FLUSH_MS);
window.addEventListener("pagehide", () => flushBattery(false, true));
window.addEventListener("beforeunload", () => flushBattery(false, true));

// ---------------------------------------------------------------- states

const slotsEl = $("slots");

async function refreshSlots() {
  const used = new Map();
  if (store && stats.crc) {
    try {
      for (const s of await store.listStates(stats.crc)) used.set(s.slot, s);
    } catch (err) {
      console.error("[nes] listStates failed:", err);
    }
  }
  slotsEl.replaceChildren();
  for (let slot = 1; slot <= SLOTS; slot++) {
    const li = document.createElement("li");
    li.dataset.slot = String(slot);
    li.classList.toggle("active", slot === stats.slot);
    const info = used.get(slot);
    const title = document.createElement("span");
    title.textContent = `Slot ${slot}`;
    const when = document.createElement("span");
    when.className = "when";
    when.textContent = info ? `${formatTime(info.at)}${info.core && info.core !== stats.core ? ` v${info.core}` : ""}` : "empty";
    li.append(title, " ", when);
    li.addEventListener("click", () => setSlot(slot));
    slotsEl.append(li);
  }
}

function formatTime(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Pick the current slot in the core (it toasts "Slot N (saved|empty)");
 *  the list highlight follows on the next tick. */
function setSlot(slot) {
  slot = Math.min(SLOTS, Math.max(1, slot));
  if (emu) emu.set_slot(slot);
  else stats.slot = slot;
  for (const li of slotsEl.children) li.classList.toggle("active", Number(li.dataset.slot) === slot);
}

const prevSlot = () => setSlot(stats.slot <= 1 ? SLOTS : stats.slot - 1);
const nextSlot = () => setSlot(stats.slot >= SLOTS ? 1 : stats.slot + 1);

/** Save through the shared UI (the same path as F5), then persist the
 *  dirty slot at once so callers can await the store. */
async function saveState(slot = stats.slot) {
  if (!emu) return false;
  if (!store) {
    toast("No store: IndexedDB unavailable", 3000);
    return false;
  }
  emu.save_slot(slot);
  stats.slot = slot;
  const bytes = emu.slot_bytes(slot);
  if (!bytes) return false;
  try {
    emu.take_dirty_slots(); // persisted here rather than by the tick
    await store.setState(stats.crc, slot, bytes, stats.core);
    stats.slotWrites++;
    await refreshSlots();
    return true;
  } catch (err) {
    console.error("[nes] save state failed:", err);
    toast(`Save failed: ${err}`, 4000);
    return false;
  }
}

/** Load through the shared UI (the same path as F8) from the slot
 *  cache, which mirrors the store. */
async function loadState(slot = stats.slot) {
  if (!emu) return false;
  const had = emu.slot_bytes(slot) !== undefined;
  emu.load_slot(slot);
  stats.slot = slot;
  clearAudio();
  present();
  setSlot(slot);
  return had;
}

async function deleteState(slot = stats.slot) {
  if (!emu || !store) return false;
  await store.deleteState(stats.crc, slot);
  emu.clear_slot_cache(slot);
  toast(`Deleted slot ${slot}`);
  await refreshSlots();
  return true;
}

$("btn-save").addEventListener("click", () => saveState());
$("btn-load").addEventListener("click", () => loadState());
$("btn-delete-state").addEventListener("click", () => deleteState());

// ---------------------------------------------------------------- cheats

const cheatListEl = $("cheat-list");

/** Parse .cht text into `[{ code, enabled, description }]`, skipping
 *  comments and blank lines (the wrapper's cheats_text has a header). */
function parseCheatLines(text) {
  const out = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/\r$/, "");
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    const [code, enabled = "1", description = ""] = line.split("\t", 3);
    out.push({ code: code.trim(), enabled: enabled.trim() !== "0", description: description.trim() });
  }
  return out;
}

function cheatLine({ code, enabled, description }) {
  const clean = (s) => String(s ?? "").replace(/[\t\r\n]+/g, " ").trim();
  return `${clean(code)}\t${enabled ? 1 : 0}\t${clean(description)}\n`;
}

function currentCheats() {
  return emu ? parseCheatLines(emu.cheats_text()) : [];
}

/** Apply a cheat list to the emulator and persist it. Returns the
 *  wrapper's error string on a bad code, else null. */
async function applyCheats(list) {
  if (!emu) return "no ROM loaded";
  const text = list.map(cheatLine).join("");
  try {
    emu.set_cheats_text(text);
  } catch (err) {
    // The wrapper reports the line of the whole text; the list index is
    // meaningless to the user, so keep the reason only.
    return String(err).replace(/^line \d+: /, "");
  }
  stats.cheatsSeeded = false;
  if (store) {
    try {
      await store.setCheats(stats.crc, emu.cheats_text());
    } catch (err) {
      console.error("[nes] cheat save failed:", err);
      toast(`Cheat save failed: ${err}`, 4000);
    }
  }
  renderCheats();
  return null;
}

async function addCheat(code, description = "") {
  const list = currentCheats();
  list.push({ code, enabled: true, description });
  const err = await applyCheats(list);
  if (err) toast(`Bad code: ${err}`, 4000);
  else toast(`Added ${code.trim().toUpperCase()}`);
  return err;
}

async function toggleCheat(index, enabled) {
  const list = currentCheats();
  if (!list[index]) return "no such cheat";
  list[index].enabled = enabled ?? !list[index].enabled;
  const err = await applyCheats(list);
  if (err) toast(`Cheat error: ${err}`, 4000);
  else toast(`${list[index].code} ${list[index].enabled ? "on" : "off"}`);
  return err;
}

async function deleteCheat(index) {
  const list = currentCheats();
  const [gone] = list.splice(index, 1);
  if (!gone) return "no such cheat";
  const err = await applyCheats(list);
  if (err) toast(`Cheat error: ${err}`, 4000);
  else toast(`Removed ${gone.code}`);
  return err;
}

function renderCheats() {
  const list = currentCheats();
  cheatListEl.replaceChildren();
  if (!emu) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "Load a ROM to edit its cheats.";
    cheatListEl.append(li);
    return;
  }
  if (list.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No cheats for this ROM.";
    cheatListEl.append(li);
  }
  list.forEach((c, i) => {
    const li = document.createElement("li");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = c.enabled;
    box.title = c.enabled ? "Disable" : "Enable";
    box.addEventListener("change", () => toggleCheat(i, box.checked));
    const code = document.createElement("code");
    code.textContent = c.code;
    const desc = document.createElement("span");
    desc.className = "desc";
    desc.textContent = c.description;
    desc.title = c.description;
    const del = document.createElement("button");
    del.type = "button";
    del.textContent = "Delete";
    del.addEventListener("click", () => deleteCheat(i));
    li.append(box, code, desc, del);
    cheatListEl.append(li);
  });
  $("cheats-note").textContent = stats.cheatsSeeded
    ? "From the bundled database (all off); the first edit stores your copy."
    : "Game Genie (6 or 8 letters), AAAA:VV, AAAA?CC:VV; join parts with +.";
}

$("cheat-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const codeEl = $("cheat-code");
  const descEl = $("cheat-desc");
  if (!emu) {
    toast("Load a ROM first");
    return;
  }
  if (!codeEl.value.trim()) return;
  const err = await addCheat(codeEl.value, descEl.value);
  if (!err) {
    codeEl.value = "";
    descEl.value = "";
  }
  codeEl.focus();
});

// ---------------------------------------------------------------- export

async function exportStore() {
  if (!store) {
    toast("No store: IndexedDB unavailable", 3000);
    return null;
  }
  await flushBattery(false);
  const doc = await store.exportJson();
  const json = JSON.stringify(doc);
  const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([json], { type: "application/json" }));
  a.download = `nes-emu-store-${stamp}.json`;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
  toast(`Exported ${Object.keys(doc.roms).length} ROM(s)`);
  return json;
}

/** Merge an export (object or JSON text) into the store, then re-apply
 *  battery RAM and cheats to the loaded ROM so imported saves take
 *  effect without a reload. */
async function importStore(docOrText) {
  if (!store) {
    toast("No store: IndexedDB unavailable", 3000);
    return null;
  }
  let counts;
  try {
    const doc = typeof docOrText === "string" ? JSON.parse(docOrText) : docOrText;
    counts = await store.importJson(doc);
  } catch (err) {
    console.error("[nes] import failed:", err);
    toast(`Import failed: ${err}`, 4000);
    return null;
  }
  if (emu) {
    const saved = emu.battery_backed() ? await store.getBattery(stats.crc) : null;
    if (saved) {
      if (emu.set_battery(saved)) console.log("[nes] battery re-applied after import");
      else console.warn("[nes] imported battery RAM does not fit this board");
    }
    const text = await store.getCheats(stats.crc);
    if (text !== null) {
      try {
        emu.set_cheats_text(text);
        stats.cheatsSeeded = false;
      } catch (err) {
        toast(`Imported cheats rejected: ${err}`, 4000);
      }
    }
    await refreshSlots();
    renderCheats();
  }
  toast(`Imported ${counts.roms} ROM(s), ${counts.states} state(s), ${counts.battery} battery, ${counts.cheats} cheat file(s)`, 3000);
  return counts;
}

$("btn-export").addEventListener("click", exportStore);
$("btn-flush").addEventListener("click", async () => {
  if (!emu) return toast("Load a ROM first");
  if (!emu.battery_backed()) return toast("This board has no battery");
  toast((await flushBattery(true)) ? "Battery saved" : "Battery save failed", 2000);
});
$("import-input").addEventListener("change", async (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  if (file) await importStore(await file.text());
});

$("rom-input").addEventListener("change", (e) => {
  const file = e.target.files[0];
  if (file) loadRomFile(file);
  e.target.value = "";
});
gate.addEventListener("click", () => $("rom-input").click());
for (const el of [wrap, document.body]) {
  el.addEventListener("dragover", (e) => {
    e.preventDefault();
    gate.classList.add("drag");
  });
  el.addEventListener("dragleave", () => gate.classList.remove("drag"));
  el.addEventListener("drop", (e) => {
    e.preventDefault();
    gate.classList.remove("drag");
    const file = e.dataTransfer.files[0];
    if (file) loadRomFile(file);
  });
}

// ---------------------------------------------------------------- input

function setPaused(paused) {
  if (!emu) return;
  emu.set_paused(paused);
  syncControls();
}

function setMuted(muted) {
  if (!emu) return;
  if (emu.muted() !== muted) emu.toggle_mute();
  syncControls();
}

function reset() {
  if (!emu) return;
  emu.reset();
  clearAudio();
}

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else wrap.requestFullscreen?.();
}

/** Browsers may leave the context suspended until a user gesture on
 *  the page; any key or click is one, so retry then. */
function resumeAudioOnGesture() {
  if (audio.ctx && audio.ctx.state !== "running") {
    audio.ctx.resume().then(() => clearAudio()).catch(() => {});
  }
}
window.addEventListener("pointerdown", resumeAudioOnGesture);

/** Keys typed into the cheat form (or any text field) must not reach
 *  the controller mapping. */
function typing(e) {
  const t = e.target;
  return t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA") && t.type !== "checkbox";
}

/** A key press by KeyboardEvent.code: the shared UI first (palette,
 *  pages, hotkeys; it consumes everything while a page is open, and
 *  F5-F8 only with a ROM so an empty page still reloads on F5), then
 *  the controller table, then F for fullscreen. Returns true when the
 *  key was used. */
function keyDown(code) {
  if (emu && emu.key_down(code)) {
    syncControls();
    return true;
  }
  const map = KEYS[code];
  if (map && emu) {
    emu.set_button(map[0], map[1], true);
    return true;
  }
  if (code === "KeyF") {
    toggleFullscreen();
    return true;
  }
  return false;
}

function keyUp(code) {
  if (emu) emu.key_up(code);
  const map = KEYS[code];
  if (map && emu) {
    emu.set_button(map[0], map[1], false);
    return true;
  }
  return false;
}

window.addEventListener("keydown", (e) => {
  resumeAudioOnGesture();
  if (e.repeat || typing(e)) return;
  if (keyDown(e.code)) e.preventDefault();
});
window.addEventListener("keyup", (e) => {
  if (typing(e)) return;
  if (keyUp(e.code)) e.preventDefault();
});
window.addEventListener("blur", () => emu?.release_all());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    emu?.release_all();
    flushBattery(false);
  } else {
    clearAudio();
  }
});

$("btn-reset").addEventListener("click", reset);
$("btn-pause").addEventListener("click", () => setPaused(!stats.paused));
$("btn-mute").addEventListener("click", () => setMuted(!stats.muted));
$("btn-crop").addEventListener("click", () => setCrop(!stats.crop));
$("btn-full").addEventListener("click", toggleFullscreen);
// Buttons must not keep focus, or Enter/Space would re-trigger them.
for (const b of document.querySelectorAll("button")) {
  b.addEventListener("click", () => b.blur());
}

// Test hooks: load a ROM from bytes without a file picker, and drive the
// store, slots, cheats and export/import without the DOM
// (docs/debugging/WASM_WEB_STORE.md).
window.nesLoadRom = (bytes, name = "rom.nes") => loadRomFile(new File([bytes], name));
window.nesApp = {
  get emu() {
    return emu;
  },
  get store() {
    return store;
  },
  flushBattery,
  saveState,
  loadState,
  deleteState,
  setSlot,
  prevSlot,
  nextSlot,
  refreshSlots,
  cheats: currentCheats,
  addCheat,
  toggleCheat,
  deleteCheat,
  applyCheats,
  exportJson: async () => (store ? store.exportJson() : null),
  importStore,
  runFrames(n) {
    if (!emu) return 0;
    for (let i = 0; i < n; i++) {
      emu.run_frame();
      emu.take_audio();
    }
    present();
    return n;
  },
  /** FNV-1a over the current frame, for comparing frames from scripts. */
  frameHash() {
    if (!emu) return null;
    const px = emu.frame_rgba();
    let h = 0x811c9dc5;
    for (let i = 0; i < px.length; i++) h = Math.imul(h ^ px[i], 0x01000193) >>> 0;
    return h.toString(16).padStart(8, "0");
  },
  setPaused,
  // Shared UI hooks (docs/plans/SHARED_OVERLAY_UI.md).
  keyDown,
  keyUp,
  presentOverlay,
  persistDirty,
  syncControls,
  /** The toast the core is showing, or "" (replaces reading #toast). */
  osdText() {
    return (emu && emu.osd_text()) || "";
  },
  /** Count of overlay pixels with non-zero alpha, from the canvas. */
  overlayAlphaPixels() {
    const data = overlay2d.getImageData(0, 0, overlay.width, overlay.height).data;
    let n = 0;
    for (let i = 3; i < data.length; i += 4) if (data[i] > 0) n++;
    return n;
  },
  /** The frame with the overlay composited, as a PNG data URL. */
  composite() {
    const out = document.createElement("canvas");
    out.width = overlay.width;
    out.height = overlay.height;
    const c = out.getContext("2d");
    c.imageSmoothingEnabled = false;
    c.drawImage(canvas, 0, 0, out.width, out.height);
    c.drawImage(overlay, 0, 0);
    return out.toDataURL("image/png");
  },
};

// ---------------------------------------------------------------- boot

await init();
stats.core = core_version();
applyCrop(true);
try {
  store = await openStore();
  window.nesStore = store;
  console.log(`[nes] store ${store.name} open`);
} catch (err) {
  console.warn("[nes] IndexedDB unavailable, nothing will persist:", err);
  $("states-note").textContent = `IndexedDB unavailable: ${err}`;
}
renderCheats();
await refreshSlots();
console.log(`[nes] core ${stats.core} ready`);
