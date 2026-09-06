// Browser-side store for battery RAM, save-state slots and cheat text
// (issue #51, docs/debugging/WASM_WEB_STORE.md). Everything is keyed by
// the ROM's CRC-32 as eight upper-case hex digits, the same key the
// bundled cheat database uses.
//
// Layout: one IndexedDB database with four object stores so every write
// is a single put with no read-modify-write (the pagehide flush must
// issue its request synchronously from the handler):
//
//   roms     { crc, name, seenAt }
//   battery  { crc, bytes: Uint8Array, at }
//   states   { key: "CRC:slot", crc, slot, bytes: Uint8Array, at, core }
//   cheats   { crc, text, at }
//
// The pure helpers (base64, export encode and decode) have no DOM or
// IndexedDB dependency so `node --test` covers them; nothing here touches
// `indexedDB` until `openStore` is called.

export const DB_NAME = "nes-emu";
export const DB_VERSION = 1;
export const SLOTS = 9;
export const EXPORT_FORMAT = "nes-emu-web-store";
export const EXPORT_VERSION = 1;

// ---------------------------------------------------------------- pure

/** Eight upper-case hex digits for a u32 CRC. */
export function crcKey(crc) {
  return (crc >>> 0).toString(16).toUpperCase().padStart(8, "0");
}

export function bytesToBase64(bytes) {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(text) {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/** Turn the record lists into the portable JSON object (bytes base64). */
export function encodeExport({ roms = [], battery = [], states = [], cheats = [] }, now = Date.now()) {
  const out = {};
  const rom = (crc) => (out[crc] ??= { name: null, battery: null, cheats: null, states: {} });
  for (const r of roms) {
    rom(r.crc).name = r.name ?? null;
  }
  for (const b of battery) {
    rom(b.crc).battery = { bytes: bytesToBase64(b.bytes), at: b.at ?? null };
  }
  for (const c of cheats) {
    rom(c.crc).cheats = { text: c.text, at: c.at ?? null };
  }
  for (const s of states) {
    rom(s.crc).states[String(s.slot)] = { bytes: bytesToBase64(s.bytes), at: s.at ?? null, core: s.core ?? null };
  }
  return { format: EXPORT_FORMAT, version: EXPORT_VERSION, exportedAt: now, roms: out };
}

/** Inverse of `encodeExport`. Throws on a file that is not ours. */
export function decodeExport(doc) {
  if (!doc || typeof doc !== "object" || doc.format !== EXPORT_FORMAT) {
    throw new Error("not a nes-emu web store export");
  }
  if (doc.version !== EXPORT_VERSION) {
    throw new Error(`unsupported export version ${doc.version}`);
  }
  const roms = [];
  const battery = [];
  const states = [];
  const cheats = [];
  for (const [rawCrc, r] of Object.entries(doc.roms ?? {})) {
    if (!/^[0-9a-fA-F]{8}$/.test(rawCrc)) throw new Error(`bad CRC key ${rawCrc}`);
    const crc = rawCrc.toUpperCase();
    roms.push({ crc, name: r.name ?? null, seenAt: null });
    if (r.battery) battery.push({ crc, bytes: base64ToBytes(r.battery.bytes), at: r.battery.at ?? null });
    if (r.cheats && typeof r.cheats.text === "string") cheats.push({ crc, text: r.cheats.text, at: r.cheats.at ?? null });
    for (const [slotText, s] of Object.entries(r.states ?? {})) {
      const slot = Number(slotText);
      if (!Number.isInteger(slot) || slot < 1 || slot > SLOTS) throw new Error(`bad slot ${slotText}`);
      states.push({ key: stateKey(crc, slot), crc, slot, bytes: base64ToBytes(s.bytes), at: s.at ?? null, core: s.core ?? null });
    }
  }
  return { roms, battery, states, cheats };
}

export function stateKey(crc, slot) {
  return `${crc}:${slot}`;
}

// ----------------------------------------------------------- IndexedDB

function request(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function done(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error("transaction aborted"));
  });
}

function upgrade(db) {
  if (!db.objectStoreNames.contains("roms")) db.createObjectStore("roms", { keyPath: "crc" });
  if (!db.objectStoreNames.contains("battery")) db.createObjectStore("battery", { keyPath: "crc" });
  if (!db.objectStoreNames.contains("cheats")) db.createObjectStore("cheats", { keyPath: "crc" });
  if (!db.objectStoreNames.contains("states")) {
    const s = db.createObjectStore("states", { keyPath: "key" });
    s.createIndex("crc", "crc", { unique: false });
  }
}

/** Open (creating on first use) and return the promise API. */
export async function openStore(name = DB_NAME, factory = globalThis.indexedDB) {
  if (!factory) throw new Error("IndexedDB is not available");
  const req = factory.open(name, DB_VERSION);
  req.onupgradeneeded = () => upgrade(req.result);
  const db = await request(req);
  db.onversionchange = () => db.close();

  const get = (store, key) => request(db.transaction(store).objectStore(store).get(key));
  const put = async (store, value) => {
    const tx = db.transaction(store, "readwrite");
    tx.objectStore(store).put(value);
    await done(tx);
  };
  const del = async (store, key) => {
    const tx = db.transaction(store, "readwrite");
    tx.objectStore(store).delete(key);
    await done(tx);
  };
  const all = (store) => request(db.transaction(store).objectStore(store).getAll());

  const api = {
    db,
    name,

    // ---- ROM index
    async touchRom(crc, romName) {
      await put("roms", { crc, name: romName, seenAt: Date.now() });
    },

    // ---- battery RAM
    async getBattery(crc) {
      const r = await get("battery", crc);
      return r ? r.bytes : null;
    },
    async setBattery(crc, bytes) {
      await put("battery", { crc, bytes: new Uint8Array(bytes), at: Date.now() });
    },
    /** Fire-and-forget put for pagehide: the request is queued before the
     *  handler returns, which is what survives the unload. */
    setBatterySync(crc, bytes) {
      const tx = db.transaction("battery", "readwrite");
      tx.objectStore("battery").put({ crc, bytes: new Uint8Array(bytes), at: Date.now() });
      return done(tx);
    },

    // ---- save states
    async getState(crc, slot) {
      return (await get("states", stateKey(crc, slot))) ?? null;
    },
    async setState(crc, slot, bytes, core) {
      await put("states", { key: stateKey(crc, slot), crc, slot, bytes: new Uint8Array(bytes), at: Date.now(), core });
    },
    async deleteState(crc, slot) {
      await del("states", stateKey(crc, slot));
    },
    /** `{ slot, at, core, size }` for every used slot of one ROM. */
    async listStates(crc) {
      const idx = db.transaction("states").objectStore("states").index("crc");
      const rows = await request(idx.getAll(crc));
      return rows.map((r) => ({ slot: r.slot, at: r.at, core: r.core, size: r.bytes.length })).sort((a, b) => a.slot - b.slot);
    },

    // ---- cheats
    async getCheats(crc) {
      const r = await get("cheats", crc);
      return r ? r.text : null;
    },
    async setCheats(crc, text) {
      await put("cheats", { crc, text, at: Date.now() });
    },
    async deleteCheats(crc) {
      await del("cheats", crc);
    },

    // ---- whole store
    async dump() {
      const [roms, battery, states, cheats] = await Promise.all([all("roms"), all("battery"), all("states"), all("cheats")]);
      return { roms, battery, states, cheats };
    },
    async exportJson() {
      return encodeExport(await api.dump());
    },
    /** Merge an export into the store; records with the same key are
     *  replaced, everything else is kept. Returns counts. */
    async importJson(doc) {
      const data = decodeExport(doc);
      const tx = db.transaction(["roms", "battery", "states", "cheats"], "readwrite");
      for (const r of data.roms) {
        const existing = tx.objectStore("roms").get(r.crc);
        existing.onsuccess = () => {
          const prev = existing.result;
          tx.objectStore("roms").put({ crc: r.crc, name: r.name ?? prev?.name ?? null, seenAt: prev?.seenAt ?? null });
        };
      }
      for (const b of data.battery) tx.objectStore("battery").put(b);
      for (const s of data.states) tx.objectStore("states").put(s);
      for (const c of data.cheats) tx.objectStore("cheats").put(c);
      await done(tx);
      return { roms: data.roms.length, battery: data.battery.length, states: data.states.length, cheats: data.cheats.length };
    },
    async clear() {
      const tx = db.transaction(["roms", "battery", "states", "cheats"], "readwrite");
      for (const s of ["roms", "battery", "states", "cheats"]) tx.objectStore(s).clear();
      await done(tx);
    },
    close() {
      db.close();
    },
  };
  return api;
}
