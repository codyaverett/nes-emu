//! What the overlay UI needs from the world outside the emulator: save
//! state slots and the cheat file.
//!
//! The SDL binary keeps them next to the ROM (`<rom>.s1` .. `.s9` and
//! `<rom>.cht`, [`FileHost`]); the web page keeps nine in-memory slots
//! and mirrors them to IndexedDB (`web/src/host.rs`). Nothing else under
//! `src/ui/` touches the file system or the clock, which is what lets
//! the same palette and pages compile for `wasm32-unknown-unknown`.

/// What the States page shows about one used slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    /// When the slot was written, seconds since the Unix epoch; `None`
    /// when the host cannot tell.
    pub modified_unix_secs: Option<u64>,
    /// Size of the state image in bytes.
    pub size: u64,
}

pub trait Host {
    /// Store `image` as slot `slot` (1-based), replacing any previous one.
    fn write_state(&mut self, slot: u8, image: &[u8]) -> Result<(), String>;

    /// The image in slot `slot`, `Ok(None)` when the slot is empty.
    fn read_state(&mut self, slot: u8) -> Result<Option<Vec<u8>>, String>;

    /// Metadata for slot `slot`, `None` when the slot is empty.
    fn slot_info(&self, slot: u8) -> Option<SlotInfo>;

    /// How the States page names the slot (`mario.s3`, `slot 3`).
    fn slot_label(&self, slot: u8) -> String;

    /// Persist the cheat set in `.cht` text form.
    fn write_cheats(&mut self, text: &str) -> Result<(), String>;

    /// How the Cheats page names the cheat store (`mario.cht`).
    fn cheats_label(&self) -> String;
}

/// Files next to the ROM, as the SDL binary has always kept them.
#[cfg(not(target_arch = "wasm32"))]
pub struct FileHost {
    rom_path: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileHost {
    pub fn new(rom_path: std::path::PathBuf) -> Self {
        FileHost { rom_path }
    }

    /// `<rom>.sN` for slot `slot`.
    pub fn state_path(&self, slot: u8) -> std::path::PathBuf {
        self.rom_path.with_extension(format!("s{slot}"))
    }

    /// `<rom>.cht`.
    pub fn cheat_path(&self) -> std::path::PathBuf {
        self.rom_path.with_extension("cht")
    }

    fn file_name(path: &std::path::Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Host for FileHost {
    fn write_state(&mut self, slot: u8, image: &[u8]) -> Result<(), String> {
        let path = self.state_path(slot);
        std::fs::write(&path, image).map_err(|e| format!("write {}: {}", path.display(), e))
    }

    fn read_state(&mut self, slot: u8) -> Result<Option<Vec<u8>>, String> {
        let path = self.state_path(slot);
        match std::fs::read(&path) {
            Ok(image) => Ok(Some(image)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {}: {}", path.display(), e)),
        }
    }

    fn slot_info(&self, slot: u8) -> Option<SlotInfo> {
        let meta = std::fs::metadata(self.state_path(slot)).ok()?;
        let modified_unix_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        Some(SlotInfo {
            modified_unix_secs,
            size: meta.len(),
        })
    }

    fn slot_label(&self, slot: u8) -> String {
        Self::file_name(&self.state_path(slot))
    }

    fn write_cheats(&mut self, text: &str) -> Result<(), String> {
        let path = self.cheat_path();
        std::fs::write(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))
    }

    fn cheats_label(&self) -> String {
        Self::file_name(&self.cheat_path())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nes-emu-host-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_host_names_and_round_trips_slots() {
        let dir = scratch("slots");
        let mut host = FileHost::new(dir.join("game.nes"));
        assert_eq!(host.slot_label(3), "game.s3");
        assert_eq!(host.cheats_label(), "game.cht");
        assert_eq!(host.state_path(9), dir.join("game.s9"));
        assert_eq!(host.read_state(1).unwrap(), None);
        assert_eq!(host.slot_info(1), None);

        host.write_state(1, b"NESS-image").unwrap();
        assert_eq!(
            host.read_state(1).unwrap().as_deref(),
            Some(&b"NESS-image"[..])
        );
        let info = host.slot_info(1).unwrap();
        assert_eq!(info.size, 10);
        assert!(info.modified_unix_secs.unwrap() > 1_700_000_000);

        host.write_cheats("SXIOPO\t1\tlives\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("game.cht")).unwrap(),
            "SXIOPO\t1\tlives\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_host_reports_write_errors() {
        let mut host = FileHost::new(std::path::PathBuf::from("/nonexistent-dir/game.nes"));
        assert!(host.write_state(1, b"x").is_err());
        assert!(host.write_cheats("x").is_err());
        assert_eq!(host.read_state(1).unwrap(), None);
    }
}
