# Mapper Trait Refactor and PPU CHR Routing

**Date:** 2026-09-04
**Type:** Architectural Refactor
**Severity:** Critical Bug Fix (CHR bank switching never reached the PPU)
**Status:** Complete
**Tracking:** GitHub issue #2, Phase 2 of docs/plans/ACCURACY_ROADMAP.md

## Executive Summary

Before this change `System::load_cartridge` copied the first 8 KB of CHR ROM
into a private PPU array once, at load time. Every mapper CHR bank write
(MMC1 `$A000/$C000`, MMC3 `$8001`, CNROM `$8000`) updated cartridge state
that nothing ever read, so any game with more than 8 KB of CHR rendered every
tile from bank 0. Mirroring was likewise copied once from the iNES header.

The cartridge is now a boxed `Mapper` trait object. The PPU fetches every
pattern-table byte through it, writes CHR RAM through it, and asks it for the
current mirroring on every nametable access.

## Background

### Symptoms (headless, 256x240 frame dumps, before vs after)

| ROM | Board | Before | After |
|-----|-------|--------|-------|
| TMNT | MMC3, CHR ROM | Multicoloured garbage where the logo should be | Correct title screen and in-game overworld |
| Contra | UxROM, CHR RAM | Blank blue screen | Correct title screen |
| SMB3 | MMC3, CHR ROM | Blank blue screen | Opening curtain; bottom half black until the MMC3 IRQ lane lands (#3) |
| mario.nes | NROM | Title screen | Byte-identical to before |
| Zelda / Final Fantasy | MMC1, CHR RAM | Blank / partial | Byte-identical to before (the old PPU array already behaved as unbanked CHR RAM); whatever blocks them is not CHR |

### Why the dead code did not help

`src/cartridge/mapper4.rs` (MMC3) and `mapper5.rs` (MMC5) existed but were
never referenced. The inline `match self.mapper` in `cartridge/mod.rs` only
knew mappers 0, 1 and 65; everything else took the "unknown mapper" fallback
that maps the last 32 KB of PRG. The MMC3 file also used addresses relative
to `$6000` and had no read arm for `$E000-$FFFF`.

## Design

### The trait (`src/cartridge/mapper.rs`)

```rust
pub trait Mapper: Send {
    fn cpu_read(&mut self, addr: u16) -> u8;        // $4020-$FFFF
    fn cpu_write(&mut self, addr: u16, value: u8);
    fn ppu_read(&mut self, addr: u16) -> u8;        // $0000-$1FFF
    fn ppu_write(&mut self, addr: u16, value: u8);
    fn mirroring(&self) -> Mirroring;
    fn irq_pending(&self) -> bool { false }
    fn clear_irq(&mut self) {}
    fn clock_scanline(&mut self) {}
}
```

Design choices:

- **Boxed, not generic.** `Cartridge` holds `Box<dyn Mapper>`; `System`
  holds `Option<Cartridge>`. One vtable call per bus access is nothing next
  to the rest of the emulator and keeps `System` a plain struct.
- **`&mut self` on reads.** MMC3 and MMC5 observe PPU address lines and MMC5
  clears its IRQ flag on `$5204` reads, so reads must be able to mutate.
- **Absolute CPU addresses.** Every mapper sees the real `$4020-$FFFF`
  address. The old code passed `addr - 0x8000`, which is why `mapper4.rs`
  had drifted to a `$6000`-relative convention.
- **PRG RAM lives in the mapper.** `System` used to serve `$6000-$7FFF`
  itself for every board. Each mapper now owns an 8 KB `prg_ram` so boards
  can gate it (MMC3 `$A001`).
- **CHR RAM lives in the mapper.** `Chr::new` allocates 8 KB of RAM when the
  header reports zero CHR banks and makes `ppu_write` effective. Bank
  arithmetic applies to RAM too, wrapping to the 8 KB.
- **`Send`.** `System` is shared with the audio thread through `Arc<Mutex>`
  in `main.rs`; the bound keeps that compiling.
- **`NullMapper`.** Used by `System` when no cartridge is loaded so the PPU
  can always be given a mapper.

### Wiring choice: pass the mapper into the PPU

Two options were considered for getting the PPU to the cartridge:

1. Give the PPU a callback or an `Rc<RefCell<dyn Mapper>>`.
2. Keep ownership in `System` and pass `&mut dyn Mapper` into
   `Ppu::step`, `Ppu::read_register` and `Ppu::write_register`.

Option 2 was chosen. It matches where CPU-side reads already live, needs no
interior mutability, and makes the borrow explicit. `System::ppu_and_mapper`
performs the split borrow:

```rust
fn ppu_and_mapper(&mut self) -> (&mut Ppu, &mut dyn Mapper) {
    let mapper: &mut dyn Mapper = match self.cartridge.as_mut() {
        Some(cart) => cart.mapper.as_mut(),
        None => &mut self.null_mapper,
    };
    (&mut self.ppu, mapper)
}
```

The parameter is threaded through `read_vram`, `write_vram`, the four
`fetch_*` pipeline functions and `fetch_sprites`. `Ppu::vram` (16 KB) became
`Ppu::nametable_ram` (4 KB): the PPU no longer holds any pattern data, and
`mirror_nametable_addr(addr, mirroring)` returns an index into that array
using the mirroring the mapper reports at that moment.

### Per-mapper files

| File | Board | Notes |
|------|-------|-------|
| `mapper0.rs` | NROM | Also the fallback for unknown mapper numbers (warning logged once at load). Oversized images expose their last 32 KB, as the old fallback did. |
| `mapper1.rs` | MMC1 | Shift register, PRG modes 0-3, CHR 4 KB/8 KB modes, mirroring from the control register. Control powers up as `0x0C` plus the header's mirroring bits so nothing changes until the game writes it. |
| `mapper2.rs` | UxROM | 16 KB switchable low bank, fixed last bank, CHR RAM. New. |
| `mapper3.rs` | CNROM | 8 KB CHR bank select. New. |
| `mapper4.rs` | MMC3 | Rewritten to absolute addresses; `clock_scanline` is byte-for-byte the old counter. `$A001` bit 7 enables PRG RAM (default enabled), bit 6 write-protects. Nothing calls `clock_scanline` yet; issue #3 wires the A12 edge and delivers the IRQ. |
| `mapper5.rs` | MMC5 | Wired for the first time. PRG/CHR modes, multiplier, ExRAM, register file. ExRAM nametables, fill mode and the vertical split are recorded but not rendered (the PPU has no per-table nametable hook); `mirroring()` approximates `$5105`. |
| `mapper65.rs` | Irem H3001 | Moved out of `mod.rs`. CHR registers corrected from `$9000-$9007` to `$B000-$B007` (nesdev); the old offset never mattered because CHR never reached the PPU. |

### Behaviour changes beyond CHR routing

- Out-of-range PRG bank reads wrap to the ROM size instead of returning 0.
  This is what the address lines do on hardware and is what the MMC3 code
  already did.
- MMC3 `$E000-$FFFF` reads now work (the dead file returned 0).
- Mapper 65 CHR register address corrected (see table).

## Verification

### Headless (`cargo test`, 48 tests)

- Mapper unit tests with synthetic bank-tagged ROMs (`tagged_rom` in
  `mapper.rs`): MMC1 shift register, reset bit, PRG modes 0-3, CHR modes,
  mirroring; MMC3 bank registers in both PRG and CHR modes, PRG RAM protect,
  mirroring, scanline counter reload/decrement/pending/acknowledge; NROM,
  UxROM, CNROM, H3001 and MMC5 bank arithmetic.
- iNES parsing: mapper number, battery flag, CHR RAM allocation, unknown
  mapper fallback, header and truncation errors.
- PPU-level tests in `src/ppu/mod.rs` that prove the wiring, not just the
  arithmetic: a `$2007` pattern read follows a CNROM bank switch, a `$2007`
  write reaches the mapper's CHR RAM, `fetch_pattern_low/high` read the
  mapper, and an MMC3 `$A000` write changes nametable mirroring for the very
  next access.
- Frame dumps: a scratch harness ran each ROM for several hundred frames
  against a build of `main` and this branch and diffed the RGB buffers (table
  above).

### Human visual pass still needed

Run the SDL binary with SMB (`mario.nes`), Zelda, SMB3 and TMNT. Expect SMB
unchanged, TMNT title and gameplay tiles correct, SMB3 curtain correct with a
black lower region until #3 lands, Zelda unchanged from before.

## How to add a mapper

1. Create `src/cartridge/mapperN.rs` with a struct holding `prg_rom`,
   `Chr` (via `Chr::new(chr_rom)`), `prg_ram` and the board's registers.
2. Implement `Mapper`. Use `prg_read(&self.prg_rom, offset)` and
   `self.chr.read(offset)` so bank offsets wrap safely. Return the live
   mirroring from `mirroring()`; override `irq_pending`, `clear_irq` and
   `clock_scanline` only if the board has an IRQ.
3. Add `pub mod mapperN;` and a match arm in `Cartridge::build_mapper`.
4. Add unit tests with `mapper::test_util::tagged_rom` so bank arithmetic is
   checked without a ROM image.
