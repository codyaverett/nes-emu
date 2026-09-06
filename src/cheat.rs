//! Cheat engine: Game Genie codes and raw address/value cheats
//! (docs/debugging/CHEAT_ENGINE.md, issue #31).
//!
//! Two kinds of cheat exist:
//!
//! - **ROM patches** intercept CPU reads in `$8000-$FFFF`. When the address
//!   matches (and, if the cheat carries a compare byte, the byte the mapper
//!   returned equals it) the cheat value is returned instead. This is
//!   exactly what a Game Genie cartridge does in hardware.
//! - **RAM freezes** poke a value into RAM once per frame, so a counter the
//!   game decrements is put back before the next frame's logic runs.
//!
//! The `System` bus hook checks [`CheatSet::is_active`] first, a single
//! bool load, so an empty or fully disabled set costs nothing on the hot
//! read path.

use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Game Genie alphabet. The index of each letter is its 4-bit value.
pub const GAME_GENIE_LETTERS: &[u8; 16] = b"APZLGITYEOXUKSVN";

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum CheatError {
    #[error("empty cheat code")]
    Empty,
    #[error("invalid Game Genie letter '{0}' (valid: APZLGITYEOXUKSVN)")]
    BadLetter(char),
    #[error("Game Genie codes are 6 or 8 letters, got {0}")]
    BadLength(usize),
    #[error("malformed raw code '{0}' (expected AAAA:VV or AAAA?CC:VV)")]
    BadRaw(String),
    #[error("line {line}: {source}")]
    Line {
        line: usize,
        #[source]
        source: Box<CheatError>,
    },
}

/// What a cheat does once decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatKind {
    /// Override a PRG ROM read at `addr` (`$8000-$FFFF`). With `compare`
    /// set, only when the real byte equals it.
    Rom {
        addr: u16,
        value: u8,
        compare: Option<u8>,
    },
    /// Write `value` to `addr` at the start of every frame.
    RamFreeze { addr: u16, value: u8 },
}

impl CheatKind {
    /// Compact raw-code rendering (`AAAA:VV` or `AAAA?CC:VV`).
    pub fn raw_code(&self) -> String {
        match *self {
            CheatKind::Rom {
                addr,
                value,
                compare: Some(cmp),
            } => format!("{addr:04X}?{cmp:02X}:{value:02X}"),
            CheatKind::Rom { addr, value, .. } | CheatKind::RamFreeze { addr, value } => {
                format!("{addr:04X}:{value:02X}")
            }
        }
    }
}

/// One cheat as the user entered it, plus its decoded effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cheat {
    /// Normalised code text: upper case, no whitespace or dashes.
    pub code: String,
    pub description: String,
    pub enabled: bool,
    /// One patch per part of the code. Multi-part codes are written with
    /// `+` between the parts (`OZTLLX+AATLGZ+SZLIVO`) and toggle together.
    pub patches: Vec<CheatKind>,
}

/// Upper-case the code and drop whitespace and dashes.
fn normalise(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn letter_value(c: char) -> Result<u8, CheatError> {
    GAME_GENIE_LETTERS
        .iter()
        .position(|&l| l as char == c)
        .map(|v| v as u8)
        .ok_or(CheatError::BadLetter(c))
}

/// Decode a normalised 6 or 8 letter Game Genie code.
///
/// Bit layout (nesdev, nesgg.txt), with `n[i]` the value of letter `i`:
///
/// ```text
/// address = 0x8000 | (n3&7)<<12 | (n5&7)<<8 | (n4&8)<<8
///                  | (n2&7)<<4  | (n1&8)<<4 | (n4&7) | (n3&8)
/// value   = (n1&7)<<4 | (n0&8)<<4 | (n0&7) | (n5&8)      6 letters
/// value   = (n1&7)<<4 | (n0&8)<<4 | (n0&7) | (n7&8)      8 letters
/// compare = (n7&7)<<4 | (n6&8)<<4 | (n6&7) | (n5&8)      8 letters
/// ```
///
/// Bit 3 of the third letter is set in 8-letter codes and clear in
/// 6-letter ones; it is informational and not checked here.
fn decode_game_genie(code: &str) -> Result<CheatKind, CheatError> {
    let n: Vec<u16> = code
        .chars()
        .map(|c| letter_value(c).map(u16::from))
        .collect::<Result<_, _>>()?;
    if n.len() != 6 && n.len() != 8 {
        return Err(CheatError::BadLength(n.len()));
    }
    let addr = 0x8000
        | ((n[3] & 7) << 12)
        | ((n[5] & 7) << 8)
        | ((n[4] & 8) << 8)
        | ((n[2] & 7) << 4)
        | ((n[1] & 8) << 4)
        | (n[4] & 7)
        | (n[3] & 8);
    let value_hi = ((n[1] & 7) << 4) | ((n[0] & 8) << 4) | (n[0] & 7);
    if n.len() == 6 {
        let value = (value_hi | (n[5] & 8)) as u8;
        Ok(CheatKind::Rom {
            addr,
            value,
            compare: None,
        })
    } else {
        let value = (value_hi | (n[7] & 8)) as u8;
        let compare = (((n[7] & 7) << 4) | ((n[6] & 8) << 4) | (n[6] & 7) | (n[5] & 8)) as u8;
        Ok(CheatKind::Rom {
            addr,
            value,
            compare: Some(compare),
        })
    }
}

/// Decode a normalised raw code: `AAAA:VV` or `AAAA?CC:VV`.
fn decode_raw(code: &str) -> Result<CheatKind, CheatError> {
    let bad = || CheatError::BadRaw(code.to_string());
    let (lhs, value) = code.split_once(':').ok_or_else(bad)?;
    let (addr, compare) = match lhs.split_once('?') {
        Some((a, c)) => (a, Some(c)),
        None => (lhs, None),
    };
    let parse_u16 = |s: &str| {
        if s.len() == 4 {
            u16::from_str_radix(s, 16).ok()
        } else {
            None
        }
    };
    let parse_u8 = |s: &str| {
        if s.len() == 2 {
            u8::from_str_radix(s, 16).ok()
        } else {
            None
        }
    };
    let addr = parse_u16(addr).ok_or_else(bad)?;
    let value = parse_u8(value).ok_or_else(bad)?;
    let compare = match compare {
        Some(c) => Some(parse_u8(c).ok_or_else(bad)?),
        None => None,
    };
    if compare.is_none() && addr < 0x8000 {
        Ok(CheatKind::RamFreeze { addr, value })
    } else {
        Ok(CheatKind::Rom {
            addr,
            value,
            compare,
        })
    }
}

impl Cheat {
    /// Parse a Game Genie code (6 or 8 letters) or a raw code (`AAAA:VV`,
    /// `AAAA?CC:VV`). Case-insensitive; whitespace and dashes are ignored.
    ///
    /// Raw codes below `$8000` without a compare byte become RAM freezes;
    /// everything else is a ROM patch.
    pub fn parse(code: &str) -> Result<Cheat, CheatError> {
        let code = normalise(code);
        if code.is_empty() {
            return Err(CheatError::Empty);
        }
        let mut patches = Vec::new();
        for part in code.split('+') {
            if part.is_empty() {
                return Err(CheatError::Empty);
            }
            patches.push(if part.contains(':') {
                decode_raw(part)?
            } else {
                decode_game_genie(part)?
            });
        }
        Ok(Cheat {
            code,
            description: String::new(),
            enabled: true,
            patches,
        })
    }

    /// The first patch; every code has at least one.
    pub fn kind(&self) -> &CheatKind {
        &self.patches[0]
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// An ordered collection of cheats with a cached "anything enabled" flag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheatSet {
    cheats: Vec<Cheat>,
    /// True when at least one cheat is enabled. Recomputed by every
    /// mutating method so the bus hook only ever tests this bool.
    active: bool,
}

impl CheatSet {
    pub fn new() -> Self {
        Self::default()
    }

    fn refresh(&mut self) {
        self.active = self.cheats.iter().any(|c| c.enabled);
    }

    /// True when at least one cheat is enabled.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn len(&self) -> usize {
        self.cheats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cheats.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cheat> {
        self.cheats.iter()
    }

    pub fn get(&self, index: usize) -> Option<&Cheat> {
        self.cheats.get(index)
    }

    /// Append a cheat and return its index.
    pub fn add(&mut self, cheat: Cheat) -> usize {
        self.cheats.push(cheat);
        self.refresh();
        self.cheats.len() - 1
    }

    /// Remove the cheat at `index`, if any.
    pub fn remove(&mut self, index: usize) -> Option<Cheat> {
        if index >= self.cheats.len() {
            return None;
        }
        let cheat = self.cheats.remove(index);
        self.refresh();
        Some(cheat)
    }

    /// Flip the enabled flag of the cheat at `index`. Returns the new
    /// state, or `None` when the index is out of range.
    pub fn toggle(&mut self, index: usize) -> Option<bool> {
        let cheat = self.cheats.get_mut(index)?;
        cheat.enabled = !cheat.enabled;
        let enabled = cheat.enabled;
        self.refresh();
        Some(enabled)
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) -> Option<()> {
        self.cheats.get_mut(index)?.enabled = enabled;
        self.refresh();
        Some(())
    }

    pub fn set_description(&mut self, index: usize, description: impl Into<String>) -> Option<()> {
        self.cheats.get_mut(index)?.description = description.into();
        Some(())
    }

    pub fn clear(&mut self) {
        self.cheats.clear();
        self.active = false;
    }

    /// Value a CPU read of `addr` should return given that the cartridge
    /// returned `rom_value`, or `None` when no enabled ROM cheat applies.
    /// The first matching cheat wins.
    pub fn rom_override(&self, addr: u16, rom_value: u8) -> Option<u8> {
        if !self.active {
            return None;
        }
        self.cheats
            .iter()
            .filter(|c| c.enabled)
            .flat_map(|c| c.patches.iter())
            .find_map(|patch| match *patch {
                CheatKind::Rom {
                    addr: a,
                    value,
                    compare,
                } if a == addr && compare.is_none_or(|cmp| cmp == rom_value) => Some(value),
                _ => None,
            })
    }

    /// Call `poke(addr, value)` for every enabled RAM freeze, in order.
    pub fn apply_ram_freezes(&self, mut poke: impl FnMut(u16, u8)) {
        if !self.active {
            return;
        }
        for c in self.cheats.iter().filter(|c| c.enabled) {
            for patch in &c.patches {
                if let CheatKind::RamFreeze { addr, value } = *patch {
                    poke(addr, value);
                }
            }
        }
    }

    /// Parse the `.cht` text format: one cheat per line as
    /// `CODE<TAB>1|0<TAB>description`; blank lines and lines starting with
    /// `#` are ignored. The enabled flag and description are optional.
    pub fn parse(text: &str) -> Result<CheatSet, CheatError> {
        let mut set = CheatSet::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let mut fields = line.splitn(3, '\t');
            let code = fields.next().unwrap_or("");
            let enabled = fields.next().map(str::trim).unwrap_or("1");
            let description = fields.next().unwrap_or("").trim().to_string();
            let mut cheat = Cheat::parse(code).map_err(|e| CheatError::Line {
                line: i + 1,
                source: Box::new(e),
            })?;
            cheat.enabled = enabled != "0";
            cheat.description = description;
            set.cheats.push(cheat);
        }
        set.refresh();
        Ok(set)
    }
}

impl fmt::Display for CheatSet {
    /// Render the `.cht` text format accepted by [`CheatSet::parse`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "# nes-emu cheats: CODE<TAB>1|0<TAB>description")?;
        for c in &self.cheats {
            writeln!(f, "{}\t{}\t{}", c.code, u8::from(c.enabled), c.description)?;
        }
        Ok(())
    }
}

/// Search `dir` for a bundled `.cht` file whose `# crc32:` header lines
/// include `crc`. Files carry one or more `# crc32: XXXXXXXX` lines so a
/// single file can cover several known dumps of a game. Returns the path
/// of the first match in name order, or `None`.
pub fn find_in_database(dir: &std::path::Path, crc: u32) -> Option<std::path::PathBuf> {
    let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "cht"))
        .collect();
    names.sort();
    names.into_iter().find(|path| {
        std::fs::read_to_string(path)
            .map(|text| database_crcs(&text).contains(&crc))
            .unwrap_or(false)
    })
}

/// Every `# crc32: XXXXXXXX` value declared in a `.cht` file's header.
pub fn database_crcs(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('#')?.trim();
            let value = rest.strip_prefix("crc32:")?.trim();
            u32::from_str_radix(value, 16).ok()
        })
        .collect()
}

impl FromStr for CheatSet {
    type Err = CheatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CheatSet::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of `decode_game_genie`, used to round-trip arbitrary
    /// address/value/compare triples through the decoder.
    fn encode(addr: u16, value: u8, compare: Option<u8>) -> String {
        let a = addr;
        let v = value as u16;
        let mut n = [0u16; 8];
        n[0] = ((v >> 4) & 8) | (v & 7);
        n[1] = ((v >> 4) & 7) | ((a >> 4) & 8);
        n[2] = ((a >> 4) & 7) | if compare.is_some() { 8 } else { 0 };
        n[3] = ((a >> 12) & 7) | (a & 8);
        n[4] = (a & 7) | ((a >> 8) & 8);
        match compare {
            None => {
                n[5] = ((a >> 8) & 7) | (v & 8);
                n[..6]
                    .iter()
                    .map(|&x| GAME_GENIE_LETTERS[x as usize] as char)
                    .collect()
            }
            Some(c) => {
                let c = c as u16;
                n[5] = ((a >> 8) & 7) | (c & 8);
                n[6] = ((c >> 4) & 8) | (c & 7);
                n[7] = ((c >> 4) & 7) | (v & 8);
                n.iter()
                    .map(|&x| GAME_GENIE_LETTERS[x as usize] as char)
                    .collect()
            }
        }
    }

    fn rom(code: &str) -> (u16, u8, Option<u8>) {
        match Cheat::parse(code).unwrap().kind().clone() {
            CheatKind::Rom {
                addr,
                value,
                compare,
            } => (addr, value, compare),
            other => panic!("{code}: expected ROM cheat, got {other:?}"),
        }
    }

    #[test]
    fn sxiopo_decodes_to_smb_infinite_lives() {
        // Documented in nesgg.txt: SXIOPO patches $91D9 (DEC $075A) to
        // $AD (LDA $075A). Verified against the SMB ROM in
        // tests/cheats_smb.rs.
        assert_eq!(rom("SXIOPO"), (0x91D9, 0xAD, None));
    }

    #[test]
    fn game_genie_is_case_and_separator_insensitive() {
        assert_eq!(rom("sxi-opo"), (0x91D9, 0xAD, None));
        assert_eq!(rom(" sx io po "), (0x91D9, 0xAD, None));
        assert_eq!(Cheat::parse("sxi-opo").unwrap().code, "SXIOPO");
    }

    #[test]
    fn eight_letter_code_carries_compare() {
        let (addr, value, compare) = rom("GZUXNGEI");
        assert_eq!(addr, 0xAC3F);
        assert_eq!(value, 0x24);
        assert_eq!(compare, Some(0xD0));
        // The address never depends on letters 7 and 8.
        let (addr6, ..) = rom("GZUXNG");
        assert_eq!(addr6, addr);
    }

    #[test]
    fn third_letter_bit3_flags_code_length() {
        // Not enforced by the decoder, but a consistency check on the
        // documented examples.
        assert_eq!(letter_value('I').unwrap() & 8, 0); // SXIOPO
        assert_eq!(letter_value('U').unwrap() & 8, 8); // GZUXNGEI
    }

    #[test]
    fn game_genie_round_trips_through_encoder() {
        assert_eq!(encode(0x91D9, 0xAD, None), "SXIOPO");
        assert_eq!(encode(0xAC3F, 0x24, Some(0xD0)), "GZUXNGEI");
        let mut seed: u32 = 0x1234_5678;
        for _ in 0..500 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let addr = 0x8000 | (seed >> 17) as u16 & 0x7FFF;
            let value = (seed >> 8) as u8;
            let compare = if seed & 1 == 1 {
                Some((seed >> 3) as u8)
            } else {
                None
            };
            let code = encode(addr, value, compare);
            assert_eq!(code.len(), if compare.is_some() { 8 } else { 6 });
            assert_eq!(rom(&code), (addr, value, compare), "code {code}");
        }
    }

    #[test]
    fn game_genie_rejects_bad_input() {
        assert_eq!(Cheat::parse(""), Err(CheatError::Empty));
        assert_eq!(Cheat::parse("SXIOP"), Err(CheatError::BadLength(5)));
        assert_eq!(Cheat::parse("SXIOPOA"), Err(CheatError::BadLength(7)));
        assert_eq!(Cheat::parse("SXIOPB"), Err(CheatError::BadLetter('B')));
    }

    #[test]
    fn raw_codes() {
        assert_eq!(
            Cheat::parse("075a:02").unwrap().kind().clone(),
            CheatKind::RamFreeze {
                addr: 0x075A,
                value: 0x02
            }
        );
        assert_eq!(rom("91D9:AD"), (0x91D9, 0xAD, None));
        assert_eq!(rom("91d9?ce:ad"), (0x91D9, 0xAD, Some(0xCE)));
        // A compare byte forces a ROM patch even below $8000.
        assert_eq!(rom("6000?01:02"), (0x6000, 0x02, Some(0x01)));
        for bad in ["91D9:", ":AD", "91D9:ADD", "9D9:AD", "91D9?C:AD", "ZZZZ:00"] {
            assert!(
                matches!(Cheat::parse(bad), Err(CheatError::BadRaw(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn rom_override_respects_compare_and_enabled() {
        let mut set = CheatSet::new();
        assert!(!set.is_active());
        assert_eq!(set.rom_override(0x91D9, 0xCE), None);

        set.add(Cheat::parse("91D9?CE:AD").unwrap());
        assert!(set.is_active());
        assert_eq!(set.rom_override(0x91D9, 0xCE), Some(0xAD));
        assert_eq!(set.rom_override(0x91D9, 0xCF), None, "compare mismatch");
        assert_eq!(set.rom_override(0x91DA, 0xCE), None, "other address");

        set.toggle(0);
        assert!(!set.is_active());
        assert_eq!(set.rom_override(0x91D9, 0xCE), None, "disabled");
        set.toggle(0);
        assert_eq!(set.rom_override(0x91D9, 0xCE), Some(0xAD));

        set.remove(0);
        assert!(set.is_empty());
        assert!(!set.is_active());
    }

    #[test]
    fn ram_freezes_only_enabled_ones() {
        let mut set = CheatSet::new();
        set.add(Cheat::parse("0010:42").unwrap());
        set.add(Cheat::parse("0011:43").unwrap());
        set.add(Cheat::parse("SXIOPO").unwrap());
        set.set_enabled(1, false);
        let mut pokes = Vec::new();
        set.apply_ram_freezes(|a, v| pokes.push((a, v)));
        assert_eq!(pokes, vec![(0x0010, 0x42)]);
    }

    #[test]
    fn cht_round_trip() {
        let mut set = CheatSet::new();
        set.add(
            Cheat::parse("SXIOPO")
                .unwrap()
                .with_description("Infinite lives"),
        );
        set.add(Cheat::parse("075a:02").unwrap());
        set.add(
            Cheat::parse("gzuxngei")
                .unwrap()
                .with_description("With compare"),
        );
        set.set_enabled(1, false);

        let text = set.to_string();
        assert!(text.starts_with('#'));
        assert!(text.contains("SXIOPO\t1\tInfinite lives\n"));
        assert!(text.contains("075A:02\t0\t\n"));
        assert!(text.contains("GZUXNGEI\t1\tWith compare\n"));

        let back: CheatSet = text.parse().unwrap();
        assert_eq!(back, set);
        assert!(back.is_active());
    }

    #[test]
    fn cht_parse_tolerates_comments_and_missing_fields() {
        let text = "# comment\n\nSXIOPO\r\n91D9:AD\t0\n  # indented comment\n";
        let set = CheatSet::parse(text).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.get(0).unwrap().enabled);
        assert!(!set.get(1).unwrap().enabled);
        assert_eq!(set.get(1).unwrap().description, "");
    }

    #[test]
    fn cht_parse_reports_line_number() {
        let err = CheatSet::parse("SXIOPO\n\nBOGUS\n").unwrap_err();
        match err {
            CheatError::Line { line, source } => {
                assert_eq!(line, 3);
                assert_eq!(*source, CheatError::BadLetter('B'));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn multi_part_codes_toggle_together() {
        let cheat = Cheat::parse("oztllx + aatlgz + szlivo").unwrap();
        assert_eq!(cheat.code, "OZTLLX+AATLGZ+SZLIVO");
        assert_eq!(cheat.patches.len(), 3);
        let mut set = CheatSet::new();
        set.add(cheat);
        let hits: Vec<u16> = (0x8000u32..=0xFFFF)
            .filter(|&a| set.rom_override(a as u16, 0x00).is_some())
            .map(|a| a as u16)
            .collect();
        assert_eq!(hits.len(), 3, "each part overrides its own address");
        set.toggle(0);
        assert!(set.rom_override(hits[0], 0x00).is_none());
        assert!(Cheat::parse("SXIOPO+").is_err());
    }
}
