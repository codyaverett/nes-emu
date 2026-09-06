// Browser front end for nes-emu (issue #50, docs/plans/WASM_WEB.md).
//
// The core runs in wasm (web/src/lib.rs). This file owns the canvas, the
// audio pipeline, input mapping and pacing. Audio is the master clock,
// as in src/main.rs: each animation frame emulates until roughly
// AUDIO_TARGET samples are queued ahead of the AudioWorklet, at most
// MAX_FRAMES_PER_TICK frames, so the display refresh rate never changes
// the game speed. Stats are exposed on window.nesStats for headless
// checks (see docs/debugging/WASM_WEB_PAGE.md).
import init, { Emulator, Button, core_version } from "./pkg/nes_emu_web.js";

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
const wrap = $("screen-wrap");
const gate = $("gate");
const toastEl = $("toast");
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
  crop: true,
  error: null,
};
window.nesStats = stats;

let emu = null;
let image = new ImageData(FRAME_W, FRAME_H);
let toastTimer = null;

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

function setCrop(crop) {
  stats.crop = crop;
  canvas.width = crop ? FRAME_W - 2 * OVERSCAN : FRAME_W;
  canvas.height = crop ? FRAME_H - 2 * OVERSCAN : FRAME_H;
  wrap.classList.toggle("full-frame", !crop);
  $("btn-crop").textContent = crop ? "Full frame" : "Crop overscan";
  if (emu) present();
}

function toast(text, ms = 1500) {
  toastEl.textContent = text;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toastEl.hidden = true), ms);
}

// ---------------------------------------------------------------- loop

let lastStatsTime = performance.now();
let framesAtLastStats = 0;
let ticksAtLastStats = 0;

function tick(now) {
  requestAnimationFrame(tick);
  if (!emu) return;
  stats.ticks++;
  if (!stats.paused) {
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
  try {
    if (emu) emu.free();
    emu = null;
    emu = new Emulator(bytes);
  } catch (err) {
    stats.error = String(err);
    toast(`Cannot load ${file.name}: ${err}`, 4000);
    console.error("[nes] load failed:", err);
    return;
  }
  stats.error = null;
  stats.rom = file.name;
  stats.frames = 0;
  stats.paused = false;
  $("btn-pause").textContent = "Pause";
  clearAudio();
  gate.hidden = true;
  document.title = `${file.name} - nes-emu`;
  console.log(`[nes] loaded ${file.name}: ${bytes.length} bytes, mapper ${emu.mapper_id()}, crc32 ${emu.rom_crc32().toString(16)}`);
  toast(`Loaded ${file.name}`);
  wrap.focus();
}

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
  stats.paused = paused;
  $("btn-pause").textContent = paused ? "Resume" : "Pause";
  toast(paused ? "Paused" : "Resumed");
  clearAudio();
}

function setMuted(muted) {
  stats.muted = muted;
  if (audio.gain) audio.gain.gain.value = muted ? 0 : 1;
  $("btn-mute").textContent = muted ? "Unmute" : "Mute";
  toast(muted ? "Muted" : "Sound on");
}

function reset() {
  if (!emu) return;
  emu.reset();
  clearAudio();
  toast("Reset");
}

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else wrap.requestFullscreen?.();
}

window.addEventListener("keydown", (e) => {
  if (e.repeat) return;
  const map = KEYS[e.code];
  if (map && emu) {
    emu.set_button(map[0], map[1], true);
    e.preventDefault();
    return;
  }
  switch (e.code) {
    case "KeyR": reset(); break;
    case "KeyP": setPaused(!stats.paused); break;
    case "KeyM": setMuted(!stats.muted); break;
    case "KeyF": toggleFullscreen(); break;
    default: return;
  }
  e.preventDefault();
});
window.addEventListener("keyup", (e) => {
  const map = KEYS[e.code];
  if (map && emu) {
    emu.set_button(map[0], map[1], false);
    e.preventDefault();
  }
});
window.addEventListener("blur", () => emu?.release_all());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) emu?.release_all();
  else clearAudio();
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

// Test hook: load a ROM from bytes without a file picker.
window.nesLoadRom = (bytes, name = "rom.nes") => loadRomFile(new File([bytes], name));

// ---------------------------------------------------------------- boot

await init();
stats.core = core_version();
setCrop(true);
console.log(`[nes] core ${stats.core} ready`);
