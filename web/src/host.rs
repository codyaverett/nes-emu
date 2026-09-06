//! The browser's `Host`: nine in-memory state slots and a cheat-text
//! dirty flag (docs/plans/SHARED_OVERLAY_UI.md, Phase 4).
//!
//! The page owns IndexedDB (`web/storage.js`), so the host never touches
//! it. Instead the page fills the slot cache from the store when a ROM
//! loads (`Emulator::set_slot_cache`), the shared UI reads and writes the
//! cache, and after every tick the page asks for the slots that changed
//! (`take_dirty_slots`, `slot_bytes`) and whether the cheat text changed
//! (`take_cheats_dirty`, then `cheats_text`) and writes those through.
//! The cache is shared between the `Host` inside the `App` and the
//! `Emulator` through an `Rc<RefCell<..>>`; wasm is single-threaded.

use std::cell::RefCell;
use std::rc::Rc;

use nes_emu::ui::app::STATE_SLOTS;
use nes_emu::ui::host::{Host, SlotInfo};

/// One cached slot: the state image and when it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedSlot {
    pub bytes: Vec<u8>,
    pub modified_unix_secs: Option<u64>,
}

/// The cache behind the host, shared with the `Emulator`.
#[derive(Default)]
pub struct SlotStore {
    slots: Vec<Option<CachedSlot>>,
    dirty: Vec<bool>,
    cheats_dirty: bool,
    /// Wall-clock seconds the page last reported, stamped on writes.
    now_unix_secs: u64,
}

pub type SharedStore = Rc<RefCell<SlotStore>>;

impl SlotStore {
    pub fn new() -> Self {
        SlotStore {
            slots: vec![None; STATE_SLOTS as usize],
            dirty: vec![false; STATE_SLOTS as usize],
            cheats_dirty: false,
            now_unix_secs: 0,
        }
    }

    fn index(slot: u8) -> Option<usize> {
        (1..=STATE_SLOTS).contains(&slot).then(|| slot as usize - 1)
    }

    pub fn set_now_unix_secs(&mut self, secs: u64) {
        self.now_unix_secs = secs;
    }

    /// Fill a slot from the page's store without marking it dirty.
    pub fn set_cached(&mut self, slot: u8, bytes: &[u8], modified_unix_secs: Option<u64>) {
        if let Some(i) = Self::index(slot) {
            self.slots[i] = Some(CachedSlot {
                bytes: bytes.to_vec(),
                modified_unix_secs,
            });
        }
    }

    /// Forget a slot (the page deleted it) without marking it dirty.
    pub fn clear_cached(&mut self, slot: u8) {
        if let Some(i) = Self::index(slot) {
            self.slots[i] = None;
            self.dirty[i] = false;
        }
    }

    pub fn bytes(&self, slot: u8) -> Option<Vec<u8>> {
        Self::index(slot).and_then(|i| self.slots[i].as_ref().map(|s| s.bytes.clone()))
    }

    /// Slots written by the UI since the last call, 1-based, ascending.
    pub fn take_dirty(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, flag) in self.dirty.iter_mut().enumerate() {
            if *flag {
                *flag = false;
                out.push(i as u8 + 1);
            }
        }
        out
    }

    pub fn take_cheats_dirty(&mut self) -> bool {
        std::mem::take(&mut self.cheats_dirty)
    }
}

pub struct WebHost {
    store: SharedStore,
}

impl WebHost {
    pub fn new(store: SharedStore) -> Self {
        WebHost { store }
    }
}

impl Host for WebHost {
    fn write_state(&mut self, slot: u8, image: &[u8]) -> Result<(), String> {
        let mut store = self.store.borrow_mut();
        let Some(i) = SlotStore::index(slot) else {
            return Err(format!("slot {slot} out of range"));
        };
        let now = store.now_unix_secs;
        store.slots[i] = Some(CachedSlot {
            bytes: image.to_vec(),
            modified_unix_secs: (now > 0).then_some(now),
        });
        store.dirty[i] = true;
        Ok(())
    }

    fn read_state(&mut self, slot: u8) -> Result<Option<Vec<u8>>, String> {
        Ok(self.store.borrow().bytes(slot))
    }

    fn slot_info(&self, slot: u8) -> Option<SlotInfo> {
        let store = self.store.borrow();
        let cached = store.slots[SlotStore::index(slot)?].as_ref()?;
        Some(SlotInfo {
            modified_unix_secs: cached.modified_unix_secs,
            size: cached.bytes.len() as u64,
        })
    }

    fn slot_label(&self, slot: u8) -> String {
        format!("slot {slot}")
    }

    fn write_cheats(&mut self, _text: &str) -> Result<(), String> {
        // The page reads the text back with `cheats_text` once it sees
        // the flag; keeping one copy avoids a second serialisation.
        self.store.borrow_mut().cheats_dirty = true;
        Ok(())
    }

    fn cheats_label(&self) -> String {
        "browser store".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_mark_slots_dirty_and_cache_reads_do_not() {
        let store: SharedStore = Rc::new(RefCell::new(SlotStore::new()));
        let mut host = WebHost::new(Rc::clone(&store));
        assert_eq!(host.read_state(1).unwrap(), None);
        assert_eq!(host.slot_info(1), None);
        assert_eq!(host.slot_label(3), "slot 3");
        assert_eq!(host.cheats_label(), "browser store");

        store
            .borrow_mut()
            .set_cached(2, b"from-db", Some(1_700_000_000));
        assert_eq!(store.borrow_mut().take_dirty(), Vec::<u8>::new());
        assert_eq!(
            host.slot_info(2),
            Some(SlotInfo {
                modified_unix_secs: Some(1_700_000_000),
                size: 7
            })
        );

        store.borrow_mut().set_now_unix_secs(1_800_000_000);
        host.write_state(1, b"NESS").unwrap();
        host.write_state(9, b"NESS9").unwrap();
        assert!(host.write_state(0, b"x").is_err());
        assert!(host.write_state(10, b"x").is_err());
        assert_eq!(store.borrow_mut().take_dirty(), vec![1, 9]);
        assert_eq!(store.borrow_mut().take_dirty(), Vec::<u8>::new());
        assert_eq!(
            host.slot_info(1).unwrap().modified_unix_secs,
            Some(1_800_000_000)
        );
        assert_eq!(store.borrow().bytes(9).as_deref(), Some(&b"NESS9"[..]));

        store.borrow_mut().clear_cached(1);
        assert_eq!(host.read_state(1).unwrap(), None);

        assert!(!store.borrow_mut().take_cheats_dirty());
        host.write_cheats("SXIOPO\t1\t\n").unwrap();
        assert!(store.borrow_mut().take_cheats_dirty());
        assert!(!store.borrow_mut().take_cheats_dirty());
    }
}
