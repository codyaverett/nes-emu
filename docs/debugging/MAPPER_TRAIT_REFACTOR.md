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
| Final Fantasy | MMC1, CHR RAM | Intro text | Byte-identical to before (the old PPU array already behaved as unbanked CHR RAM) |
| Zelda | MMC1, CHR RAM | Blank blue at 900 frames | Byte-identical to before. Rendering is enabled (`$2001` = `0x1E`) but the palette is never written, so the game has not reached its PPU init; a CPU-side or NMI-path symptom for another lane, not CHR |

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
    fn ppu_a12_rise(&mut self) {}          // MMC3 (issue 3)
    fn ppu_fetch(&mut self, addr: u16) {}  // MMC2/MMC4 latches (issue 43)
    fn cpu_clock(&mut self) {}             // FME-7 IRQ counter (issue 43)
    // plus cpu_peek/ppu_peek, prg_ram/prg_ram_mut, save_state/load_state
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
| `mapper227.rs` | Address-latch multicart (1200-in-1) | Added for issue 25, see below. |
| `mapper7.rs` | AxROM | 32 KB PRG bank, single-screen mirroring from the register, CHR RAM. Issue 43. |
| `mapper9.rs` | MMC2 (PNROM) | 8 KB PRG bank with the last three fixed, four 4 KB CHR registers picked by the two tile-$FD/$FE latches, which the PPU flips through `ppu_fetch`. Holds the `Mmc2Core` shared with MMC4. Issue 43. |
| `mapper10.rs` | MMC4 (FxROM) | `Mmc2Core` with 16 KB PRG banking and the whole-row latch 0. Issue 43. |
| `mapper11.rs` | Color Dreams | 32 KB PRG from bits 0-1, 8 KB CHR from bits 4-7. Issue 43. |
| `mapper66.rs` | GxROM | 32 KB PRG from bits 4-5, 8 KB CHR from bits 0-1. Issue 43. |
| `mapper69.rs` | Sunsoft FME-7 | Command/parameter registers, 1 KB CHR banks, 8 KB PRG banks with the `$6000` RAM/ROM slot, four mirroring modes, 16-bit IRQ counter clocked per CPU cycle through `cpu_clock`. Issue 43. |

### Mapper 227 (issue 25)

The 1200-in-1 multicart (`roms/1200-in-1.nes`, 512 KB PRG, CHR RAM) is
iNES mapper 227. The register is the CPU address of any write to
`$8000-$FFFF`; the data byte is ignored. Per nesdev "INES Mapper 227":

```text
[A~1... .mLQ OQQP PpMS]
         ||| |||| |||+- S: 0 = 16 KB mode, 1 = 32 KB mode (PRG A14 = CPU A14)
         ||| |||| ||+-- M: 0 = vertical mirroring, 1 = horizontal
         ||| |||+-++--- PPp: inner 16 KB bank (PRG A16..A14)
         ||+-|++------- QQQ: outer 128 KB bank (PRG A19..A17; bit 8 is A19)
         ||  +--------- O: 0 = UNROM-like, $C000 fixed to inner bank 0 or 7
         ||               1 = NROM: $C000 mirrors $8000 (S=0) or 32 KB bank (S=1)
         |+------------ L: inner bank fixed at $C000 when O=0 (0 -> #0, 1 -> #7)
         +------------- m: solder-pad menu select (submapper 1 only, ignored)
```

Implementation notes:

- The 16 KB bank number is `((addr >> 2) & 0x1F) | ((addr >> 3) & 0x20)`:
  bit 7 (O) sits between the two bank fields and must be skipped. Out of
  range banks wrap to the image size through `prg_read`.
- With O=0 the `$C000` bank is the outer 128 KB block's inner bank 0 (L=0)
  or 7 (L=1); the `$8000` bank is the full number, with its low bit cleared
  when S=1 (nesdev calls this "pointless" but it is what the board does).
- CHR RAM is write-protected while O=1 (the multicart variant). The Chinese
  RPG boards that keep it writable in NROM mode are not distinguished; no
  such ROM is in `roms/`.
- Power-on latch is 0: bank 0 at both halves, vertical mirroring, CHR RAM
  writable. The header mirroring bit is ignored because the latch owns it.

Unit tests (`tagged_rom(32, 0x4000)`, 64 banks for the A19 case) cover
power-on state, O=0 with L=0 and L=1 across outer blocks, O=0 with S=1
reaching only even banks, NROM-128 mirroring, NROM-256 32 KB pairs, bit 8
as the high bank bit with bit 7 not leaking in, the latch ignoring the data
byte and sub-`$8000` writes, mirroring from bit 1, and CHR RAM write
protection following O.

Frame dumps with the ROM: the menu ("1200 IN 1 / PUSH SEL . START BUTTON",
twenty titles per page, cursor on entry 10) draws at frame 200 with no
input. Select pages the list by twenty entries, Down moves the cursor one
line, and Start launches the highlighted game (Bomberman, Galaxian). Right
does nothing on this menu; it is not a navigation key there. The
compatibility sweep's fixed script (Start at 150) therefore lands in
Bomberman stage 1.

### Mappers 7, 9, 10, 11, 66, 69 (issue 43)

Six more boards for library coverage. None of the ROMs in `roms/` needed
them except `SuperMarioBros.nes`, which turned out to be the GxROM (mapper
66, 64 KB PRG, 16 KB CHR) Super Mario Bros / Duck Hunt cart and had been
running on the NROM fallback; see the verification notes below. All six
follow the `mapper2.rs`/`mapper4.rs` shape: `cpu_read` delegates to
`cpu_peek`, `ppu_peek` mirrors `ppu_read` without side effects, PRG RAM is
an 8 KB array exposed through `prg_ram`/`prg_ram_mut` even on boards that
have none (UxROM already does this), and `save_state` writes the registers
first, then PRG RAM, then `Chr::save_state`.

Two hooks were added to the trait so that the mappers, not the PPU or
`System`, own the timing-sensitive state:

- `ppu_fetch(addr)`: `Ppu::read_vram` calls it right after `mapper.ppu_read`
  for every pattern-table access, so background fetches, sprite fetches and
  `$2007` reads all report. Ordering is the point: the byte at the trigger
  address comes from the bank selected before the flip, the next fetch from
  the new one. `ppu_peek` never calls it. Existing mappers use the default
  no-op, so the fingerprints of every other ROM are unchanged.
- `cpu_clock()`: `System::tick` calls it once per CPU cycle, after the APU
  step and before `sample_irq_input`, so a counter wrap in this cycle is
  visible to the IRQ sample of the same cycle. `tick` is the only place
  `total_cycles` advances (DMA stalls go through it too), so the FME-7 count
  is exact.

#### Mapper 7 (AxROM)

One register anywhere in `$8000-$FFFF`: bits 0-2 pick the 32 KB PRG bank,
bit 4 picks the single nametable page (0 = `SingleScreenLower`, 1 =
`SingleScreenUpper`). CHR is 8 KB RAM, unbanked. The header mirroring is
ignored because the register owns it; power-on is bank 0, lower page.

#### Mappers 9 (MMC2) and 10 (MMC4)

`mapper9.rs` holds `Mmc2Core` with a `Kind` switch; `Mapper9` and
`Mapper10` are newtypes that forward every trait method through the
`forward_mmc2_core!` macro. Registers (nesdev "MMC2", "MMC4"):

```text
$A000-$AFFF  PRG bank (bits 0-3)
             MMC2: 8 KB at $8000-$9FFF, $A000-$FFFF = last three 8 KB banks
             MMC4: 16 KB at $8000-$BFFF, $C000-$FFFF = last 16 KB bank
$B000/$C000  4 KB CHR bank for $0000-$0FFF while latch 0 = $FD / $FE
$D000/$E000  4 KB CHR bank for $1000-$1FFF while latch 1 = $FD / $FE
$F000-$FFFF  mirroring bit 0: 0 vertical, 1 horizontal
```

The latches flip on PPU reads (reported through `ppu_fetch`) of the high
plane of tiles `$FD` and `$FE`:

| Trigger | MMC2 | MMC4 |
|---------|------|------|
| latch 0 = `$FD` | `$0FD8` only | `$0FD8-$0FDF` |
| latch 0 = `$FE` | `$0FE8` only | `$0FE8-$0FEF` |
| latch 1 = `$FD` | `$1FD8-$1FDF` | `$1FD8-$1FDF` |
| latch 1 = `$FE` | `$1FE8-$1FEF` | `$1FE8-$1FEF` |

The decode is an explicit match on the full address, not a mask: an early
version masked off bit 13 and a unit test caught `$2FE8` aliasing onto
`$0FE8`. Power-on latches are `$FE` (unspecified on hardware; the common
emulator choice). Both boards get 8 KB PRG RAM at `$6000` (MMC4 boards
have it, battery backed on Fire Emblem; MMC2 boards do not, but the array
is harmless).

#### Mapper 11 (Color Dreams)

One register: bits 0-1 the 32 KB PRG bank, bits 4-7 the 8 KB CHR bank, bits
2-3 are the lockout-defeat lines and ignored. Mirroring from the header.

#### Mapper 66 (GxROM)

One register: bits 4-5 the 32 KB PRG bank, bits 0-1 the 8 KB CHR bank.
Mirroring from the header. Bus conflicts are not modelled on either 11 or
66.

#### Mapper 69 (Sunsoft FME-7)

`$8000-$9FFF` selects a command (bits 0-3), `$A000-$BFFF` writes its
parameter; `$C000-$FFFF` is the 5B audio chip and ignored.

```text
$0-$7  1 KB CHR bank for $0000 + n * $400
$8     $6000-$7FFF slot: ERbB BBBB. R = 1 selects PRG RAM, readable and
       writable only while E = 1 (otherwise open bus, returned as 0);
       R = 0 maps 8 KB PRG ROM bank bBBBBB there
$9-$B  8 KB PRG ROM bank (bits 0-5) at $8000, $A000, $C000;
       $E000-$FFFF fixed to the last bank
$C     mirroring bits 0-1: 0 horizontal, 1 vertical, 2 single lower,
       3 single upper (note 0 is horizontal, the opposite of MMC1/MMC3)
$D     IRQ control: bit 0 IRQ output enable, bit 7 counter enable;
       every write acknowledges the IRQ
$E/$F  IRQ counter low / high byte
```

The 16-bit counter decrements once per `cpu_clock` while bit 7 is set and
raises the IRQ on the `$0000` to `$FFFF` wrap when bit 0 is set. It keeps
counting after the wrap and the line stays asserted until a command `$D`
write (or `clear_irq`).

#### Tests

Each file has unit tests on `tagged_rom` images: power-on layout, every
bank field including the bits that must not leak into it, wrap to the image
size, mirroring, the `$6000` slot modes and open bus (69), the IRQ counter
(exact cycle count, high byte, counter-enable and IRQ-enable independently,
acknowledge), and a save-state round trip that switches banks, saves,
switches again, loads and checks the reads match. The MMC2/MMC4 tests drive
`ppu_fetch` directly for every trigger row and a set of non-triggers, and
`src/ppu/mod.rs` has `pattern_reads_report_the_fetch_to_the_mapper_after_the_byte`,
which proves the wiring through `$2007` and `fetch_pattern_high`.

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
- Issue 43: `cargo test --release --test game_frames -- --ignored --nocapture`
  before and after. Ten of the eleven ROMs are hash-identical at every
  checkpoint, which is the expected result of a no-op default hook. The one
  change is `SuperMarioBros.nes`: its header says mapper 66, so it moved
  from the NROM fallback (last 32 KB of a 64 KB image) to `Mapper66` and now
  boots to the real title screen. Frame dumps at 240 (appendix harness,
  built once against a `git archive main` extraction and once against the
  branch): main showed the title logo half drawn with garbage tiles from
  the wrong CHR bank where the text and ground should be; the branch shows
  the standard SMB title, 1985 Nintendo line, player menu and ground tiles.

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
   one of `clock_scanline` (MMC5), `ppu_a12_rise` (MMC3) or `cpu_clock`
   (FME-7) only if the board has an IRQ. Override `ppu_fetch` only if the
   board watches the PPU address bus (MMC2/MMC4 latches); never flip state
   from `ppu_read` or `ppu_peek`.
   Implement `cpu_peek`, `ppu_peek`, `prg_ram`/`prg_ram_mut` and
   `save_state`/`load_state` (order documented in a comment above
   `save_state`, ending with PRG RAM then `Chr::save_state`); add the row
   to the `MAPR` table in docs/debugging/SAVE_STATES.md.
3. Add `pub mod mapperN;` and a match arm in `Cartridge::build_mapper`.
4. Add unit tests with `mapper::test_util::tagged_rom` so bank arithmetic is
   checked without a ROM image.

## Appendix: reproducing the frame diff

The before/after table was produced with a throwaway cargo project outside
the repo that depends on `nes-emu` by path, runs a ROM for N frames, and
dumps the raw 256x240 RGB buffer. Build it twice, once against a
`git archive main` extraction and once against the branch, then `cmp` the
dumps and convert them to PNG.

`Cargo.toml`:

```toml
[package]
name = "frame-harness"
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
nes-emu = { path = "/path/to/nes-emu" }
```

`src/main.rs`:

```rust
use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::system::System;
use std::{env, fs};

fn main() {
    let args: Vec<String> = env::args().collect();
    let rom = &args[1];
    let frames: usize = args[2].parse().unwrap();
    let out = &args[3];
    // Optional: tap Start at the given frame so title screens advance
    let start_at: Option<usize> = args.get(4).map(|s| s.parse().unwrap());

    let cart = Cartridge::load_from_file(rom).unwrap();
    let mut system = System::new();
    system.load_cartridge(cart);
    for f in 0..frames {
        if let Some(s) = start_at {
            if f == s {
                system.controller1.press(ControllerButton::START);
            } else if f == s + 10 {
                system.controller1.release(ControllerButton::START);
            }
        }
        system.run_frame();
    }
    fs::write(out, system.get_frame_buffer()).unwrap();
}
```

`topng.py` (stdlib only):

```python
import struct, sys, zlib
W, H = 256, 240
raw = open(sys.argv[1], 'rb').read()
rows = b''.join(b'\x00' + raw[y * W * 3:(y + 1) * W * 3] for y in range(H))
def chunk(t, d):
    return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
with open(sys.argv[2], 'wb') as f:
    f.write(b'\x89PNG\r\n\x1a\n')
    f.write(chunk(b'IHDR', struct.pack('>IIBBBBB', W, H, 8, 2, 0, 0, 0)))
    f.write(chunk(b'IDAT', zlib.compress(rows)))
    f.write(chunk(b'IEND', b''))
```

Frame counts used: SMB 240/600, Zelda 400/900, SMB3 500/900 (Start at 500),
TMNT 400 and 700 (Start at 400), Final Fantasy 400, Contra 300.
