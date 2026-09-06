//! Every bundled file in `cheats/` must parse, decode, and declare at least
//! one CRC-32 so the binary can match it to a ROM.

use nes_emu::cheat::{database_crcs, find_in_database, CheatSet};
use std::path::Path;

fn database_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("cheats")
}

#[test]
fn every_bundled_file_parses_and_has_a_crc() {
    let mut files = 0;
    for entry in std::fs::read_dir(database_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|x| x != "cht") {
            continue;
        }
        files += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let set: CheatSet = text
            .parse()
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
        assert!(!set.is_empty(), "{} has no cheats", path.display());
        assert!(
            set.iter().all(|c| !c.enabled),
            "{} ships with a cheat enabled",
            path.display()
        );
        assert!(
            !database_crcs(&text).is_empty(),
            "{} declares no crc32",
            path.display()
        );
    }
    assert!(files >= 9, "expected the bundled files, found {files}");
}

#[test]
fn lookup_finds_a_file_by_crc_and_ignores_unknown_ones() {
    let dir = database_dir();
    let smb = find_in_database(&dir, 0x8E2B_D25C).expect("Super Mario Bros entry");
    assert!(smb.ends_with("Super Mario Bros.cht"));
    assert!(find_in_database(&dir, 0xDEAD_BEEF).is_none());
}
