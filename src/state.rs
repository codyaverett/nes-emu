//! Save-state container format and the [`Snapshot`] trait.
//!
//! A state file is a little-endian byte image (docs/debugging/SAVE_STATES.md):
//!
//! ```text
//! magic    4 bytes  "NESS"
//! version  u16      FORMAT_VERSION
//! crc      u32      CRC-32 of the ROM image the state belongs to
//! sections repeated until end of file:
//!   tag    4 bytes  ASCII section name ("CPU ", "RAM ", "PPU ", ...)
//!   len    u32      payload length in bytes
//!   data   len bytes
//! ```
//!
//! Sections are self-delimiting so a reader can skip a tag it does not
//! know; that is how the format grows without a version bump. Inside a
//! section the field order is fixed and documented next to the `Snapshot`
//! impl that writes it. Nothing is derived: every field that affects
//! emulation is written and read by hand, and a section that is not
//! consumed to its last byte on load is an error, so a field added to one
//! side and not the other fails loudly instead of shifting everything
//! after it.
//!
//! `Box<dyn Mapper>` rules out derive-based serialisation, and the format
//! needs no crate: the whole thing is a `Vec<u8>` and a bounded slice.

use thiserror::Error;

/// Bytes 0-3 of every state file.
pub const MAGIC: [u8; 4] = *b"NESS";

/// Bumped when a section's layout changes incompatibly. Adding a new
/// section does not need a bump: unknown tags are skipped.
pub const FORMAT_VERSION: u16 = 1;

/// Section tags written by `System::save_state`, in file order.
pub const TAG_CPU: &[u8; 4] = b"CPU ";
pub const TAG_RAM: &[u8; 4] = b"RAM ";
pub const TAG_PPU: &[u8; 4] = b"PPU ";
pub const TAG_APU: &[u8; 4] = b"APU ";
pub const TAG_MAPPER: &[u8; 4] = b"MAPR";
pub const TAG_INPUT: &[u8; 4] = b"INPT";

/// Sections a state must contain to be loadable.
pub const REQUIRED_TAGS: [&[u8; 4]; 6] =
    [TAG_CPU, TAG_RAM, TAG_PPU, TAG_APU, TAG_MAPPER, TAG_INPUT];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    #[error("not a save state (bad magic)")]
    BadMagic,
    #[error("unsupported save state version {0} (this build reads {FORMAT_VERSION})")]
    UnsupportedVersion(u16),
    #[error(
        "save state is for a different ROM (state CRC-32 {found:08X}, loaded ROM {expected:08X})"
    )]
    RomMismatch { expected: u32, found: u32 },
    #[error("no cartridge loaded")]
    NoCartridge,
    #[error("save state is truncated")]
    Truncated,
    #[error("section {0:?} has {1} unread byte(s): layout mismatch")]
    TrailingBytes(String, usize),
    #[error("save state has no {0:?} section")]
    MissingSection(String),
    #[error("section {0:?}: {1}")]
    BadValue(String, String),
}

/// Something whose emulation state can be written to and restored from a
/// byte image. `load` must restore every field `save` wrote and nothing
/// else; it must not call the component's `reset`.
pub trait Snapshot {
    fn save(&self, w: &mut Writer);
    fn load(&mut self, r: &mut Reader) -> Result<(), StateError>;
}

/// Little-endian byte sink.
#[derive(Default, Debug)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Bit-exact float image (`to_le_bytes`), so a re-save after a load
    /// reproduces the same bytes.
    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Raw bytes, no length prefix; the reader must know the size.
    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// `u32` length followed by the bytes.
    pub fn blob(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.bytes(v);
    }

    /// `Option<u8>` as a presence byte then the value (0 when absent).
    pub fn opt_u8(&mut self, v: Option<u8>) {
        self.bool(v.is_some());
        self.u8(v.unwrap_or(0));
    }

    pub fn opt_bool(&mut self, v: Option<bool>) {
        self.bool(v.is_some());
        self.bool(v.unwrap_or(false));
    }

    pub fn opt_u64(&mut self, v: Option<u64>) {
        self.bool(v.is_some());
        self.u64(v.unwrap_or(0));
    }

    /// Write a tagged, length-prefixed section whose payload is produced
    /// by `f`. `tag` must be exactly four bytes.
    pub fn section(&mut self, tag: &[u8; 4], f: impl FnOnce(&mut Writer)) {
        let mut inner = Writer::new();
        f(&mut inner);
        self.buf.extend_from_slice(tag);
        self.u32(inner.buf.len() as u32);
        self.buf.extend_from_slice(&inner.buf);
    }
}

/// Bounded little-endian reader. Every read past the end is
/// [`StateError::Truncated`].
#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        if self.remaining() < n {
            return Err(StateError::Truncated);
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, StateError> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> Result<u16, StateError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, StateError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, StateError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn f32(&mut self) -> Result<f32, StateError> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f64(&mut self) -> Result<f64, StateError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(f64::from_le_bytes(a))
    }

    /// Fill `out` with exactly `out.len()` bytes.
    pub fn bytes(&mut self, out: &mut [u8]) -> Result<(), StateError> {
        let b = self.take(out.len())?;
        out.copy_from_slice(b);
        Ok(())
    }

    /// A `u32` length followed by that many bytes.
    pub fn blob(&mut self) -> Result<&'a [u8], StateError> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn opt_u8(&mut self) -> Result<Option<u8>, StateError> {
        let some = self.bool()?;
        let v = self.u8()?;
        Ok(some.then_some(v))
    }

    pub fn opt_bool(&mut self) -> Result<Option<bool>, StateError> {
        let some = self.bool()?;
        let v = self.bool()?;
        Ok(some.then_some(v))
    }

    pub fn opt_u64(&mut self) -> Result<Option<u64>, StateError> {
        let some = self.bool()?;
        let v = self.u64()?;
        Ok(some.then_some(v))
    }

    /// Read the next section header and return its tag and a reader over
    /// its payload, or `None` at the end of the data.
    pub fn section(&mut self) -> Result<Option<([u8; 4], Reader<'a>)>, StateError> {
        if self.remaining() == 0 {
            return Ok(None);
        }
        let mut tag = [0u8; 4];
        self.bytes(&mut tag)?;
        let payload = self.blob()?;
        Ok(Some((tag, Reader::new(payload))))
    }

    /// Error unless every byte of this reader has been consumed. Called
    /// after each known section is loaded so a layout mismatch between
    /// `save` and `load` cannot pass silently.
    pub fn finish(&self, tag: &[u8; 4]) -> Result<(), StateError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(StateError::TrailingBytes(tag_name(tag), self.remaining()))
        }
    }
}

/// Printable form of a section tag for error messages.
pub fn tag_name(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).trim_end().to_string()
}

/// A parsed state image: header fields and the section list, checked for
/// framing before anything is applied to the machine.
#[derive(Debug, PartialEq, Eq)]
pub struct Image<'a> {
    pub version: u16,
    pub rom_crc32: u32,
    pub sections: Vec<([u8; 4], &'a [u8])>,
}

/// Validate the header and walk every section header. A truncated file
/// or a length field past the end is refused here, before any component
/// has been touched.
pub fn parse(data: &[u8]) -> Result<Image<'_>, StateError> {
    let mut r = Reader::new(data);
    let mut magic = [0u8; 4];
    r.bytes(&mut magic).map_err(|_| StateError::BadMagic)?;
    if magic != MAGIC {
        return Err(StateError::BadMagic);
    }
    let version = r.u16()?;
    if version != FORMAT_VERSION {
        return Err(StateError::UnsupportedVersion(version));
    }
    let rom_crc32 = r.u32()?;
    let mut sections = Vec::new();
    while let Some((tag, payload)) = r.section()? {
        sections.push((tag, payload.data));
    }
    Ok(Image {
        version,
        rom_crc32,
        sections,
    })
}

/// Start a state image: magic, version and ROM CRC.
pub fn header(rom_crc32: u32) -> Writer {
    let mut w = Writer::new();
    w.bytes(&MAGIC);
    w.u16(FORMAT_VERSION);
    w.u32(rom_crc32);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_round_trip_little_endian() {
        let mut w = Writer::new();
        w.u8(0x12);
        w.bool(true);
        w.u16(0x3456);
        w.u32(0x789A_BCDE);
        w.u64(0x0102_0304_0506_0708);
        w.f32(1.5);
        w.f64(-2.25);
        w.opt_u8(Some(7));
        w.opt_u8(None);
        w.opt_bool(Some(false));
        w.opt_u64(Some(9));
        w.blob(&[1, 2, 3]);
        let bytes = w.into_bytes();
        assert_eq!(&bytes[..7], &[0x12, 1, 0x56, 0x34, 0xDE, 0xBC, 0x9A]);

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 0x12);
        assert!(r.bool().unwrap());
        assert_eq!(r.u16().unwrap(), 0x3456);
        assert_eq!(r.u32().unwrap(), 0x789A_BCDE);
        assert_eq!(r.u64().unwrap(), 0x0102_0304_0506_0708);
        assert_eq!(r.f32().unwrap(), 1.5);
        assert_eq!(r.f64().unwrap(), -2.25);
        assert_eq!(r.opt_u8().unwrap(), Some(7));
        assert_eq!(r.opt_u8().unwrap(), None);
        assert_eq!(r.opt_bool().unwrap(), Some(false));
        assert_eq!(r.opt_u64().unwrap(), Some(9));
        assert_eq!(r.blob().unwrap(), &[1, 2, 3]);
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.u8(), Err(StateError::Truncated));
    }

    #[test]
    fn sections_are_tagged_and_length_prefixed() {
        let mut w = header(0xDEAD_BEEF);
        w.section(b"ONE ", |w| w.u16(1));
        w.section(b"TWO ", |w| w.bytes(&[9; 5]));
        let bytes = w.into_bytes();
        assert_eq!(&bytes[..4], b"NESS");
        assert_eq!(&bytes[10..14], b"ONE ");
        assert_eq!(&bytes[14..18], &[2, 0, 0, 0]);

        let image = parse(&bytes).unwrap();
        assert_eq!(image.version, FORMAT_VERSION);
        assert_eq!(image.rom_crc32, 0xDEAD_BEEF);
        assert_eq!(image.sections.len(), 2);
        assert_eq!(&image.sections[0].0, b"ONE ");
        assert_eq!(image.sections[0].1, &[1, 0]);
        assert_eq!(&image.sections[1].0, b"TWO ");
        assert_eq!(image.sections[1].1.len(), 5);
    }

    #[test]
    fn parse_refuses_bad_magic_version_and_truncation() {
        assert_eq!(
            parse(b"NESX\x01\x00\x00\x00\x00\x00"),
            Err(StateError::BadMagic)
        );
        assert_eq!(parse(b"NES"), Err(StateError::BadMagic));
        assert_eq!(
            parse(b"NESS\x09\x00\x00\x00\x00\x00"),
            Err(StateError::UnsupportedVersion(9))
        );
        let mut w = header(1);
        w.section(b"ONE ", |w| w.u32(5));
        let bytes = w.into_bytes();
        // A bare header parses as a state with no sections; every other
        // proper prefix is truncated somewhere.
        assert!(parse(&bytes[..10]).is_ok());
        for cut in (4..10).chain(11..bytes.len()) {
            assert_eq!(
                parse(&bytes[..cut]),
                Err(StateError::Truncated),
                "cut {cut}"
            );
        }
        // A length field that overruns the file.
        let mut bad = bytes.clone();
        bad[14] = 0xFF;
        assert_eq!(parse(&bad), Err(StateError::Truncated));
    }

    #[test]
    fn finish_reports_unread_bytes() {
        let bytes = [1u8, 2, 3];
        let mut r = Reader::new(&bytes);
        r.u8().unwrap();
        assert_eq!(
            r.finish(b"CPU "),
            Err(StateError::TrailingBytes("CPU".into(), 2))
        );
        r.u16().unwrap();
        assert_eq!(r.finish(b"CPU "), Ok(()));
    }
}
