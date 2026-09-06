# Cheat Engine: Game Genie and Raw Codes in the Library

**Date:** 2026-09-05
**Type:** Feature
**Issue:** #31 (Phase 2 of docs/plans/TOOLS_AND_CHEATS.md)
**Status:** Complete

## Executive Summary

`src/cheat.rs` decodes Game Genie codes and raw address/value codes into
two kinds of cheat, and `System` applies them at two hook points: ROM
patches intercept CPU reads of `$8000-$FFFF`, RAM freezes are poked once
per frame. The set is persisted as a `.cht` text file next to the ROM.
The library stays UI-free; the tool page and palette commands are issue
#32.

## Code formats

All input is upper-cased and stripped of whitespace and dashes before
decoding, so `sxi-opo`, `SXIOPO` and `sxi opo` are the same code.

| Form          | Example        | Meaning                                              |
|---------------|----------------|------------------------------------------------------|
| 6 letters     | `SXIOPO`       | ROM patch, no compare byte                           |
| 8 letters     | `GZUXNGEI`     | ROM patch with compare byte                          |
| `AAAA:VV`     | `075A:02`      | RAM freeze when AAAA < `$8000`, else ROM patch       |
| `AAAA?CC:VV`  | `91D9?CE:AD`   | ROM patch that only applies when the ROM byte is CC |

Game Genie letters and their values: `A P Z L G I T Y E O X U K S V N`
= 0 to 15. Any other letter is `CheatError::BadLetter`; lengths other
than 6 or 8 are `CheatError::BadLength`.

### Game Genie decoding

With `n0..n7` the values of the letters in order (nesdev wiki, nesgg.txt):

```text
address = 0x8000 | (n3&7)<<12 | (n5&7)<<8 | (n4&8)<<8
                 | (n2&7)<<4  | (n1&8)<<4 | (n4&7) | (n3&8)
value   = (n1&7)<<4 | (n0&8)<<4 | (n0&7) | (n5&8)      6 letters
value   = (n1&7)<<4 | (n0&8)<<4 | (n0&7) | (n7&8)      8 letters
compare = (n7&7)<<4 | (n6&8)<<4 | (n6&7) | (n5&8)      8 letters
```

The address only ever uses letters 2 to 6, so an 8-letter code and its
first six letters patch the same address. Bit 3 of the third letter is
set in 8-letter codes and clear in 6-letter ones; the decoder does not
enforce it (it uses the length), but the unit tests check the documented
examples agree.

Worked example, `SXIOPO` (Super Mario Bros infinite lives):

```text
S=13 X=10 I=5 O=9 P=1 O=9
address = 0x8000 | 1<<12 | 1<<8 | 0 | 5<<4 | 8<<4 | 1 | 8 = 0x91D9
value   = 2<<4 | 8<<4 | 5 | 8 = 0xAD
```

nesgg.txt lists the same result. Checking the ROM confirms the mechanism:
`$91D9` in mario.nes holds `CE 5A 07` (`DEC $075A`, the lives decrement
in the player-death routine); `$AD` turns it into `LDA $075A`, a harmless
load with the same operand bytes.

`GZUXNGEI` decodes to `$AC3F`, value `$24`, compare `$D0`. Neither
source we could reach carries a decoded value for it, so the unit tests
verify it by round-tripping through an encoder (the inverse bit layout)
and by checking its address equals that of `GZUXNG`.

## Hook points

### ROM patches: `System::bus_read`

The `$4020-$FFFF` arm reads the byte from the mapper first (compare needs
the real byte and mapper side effects must still happen), then, only when
`CheatSet::is_active()` and `addr >= 0x8000`, calls
`CheatSet::rom_override(addr, value)`. ROM patches therefore only ever
apply to reads at `$8000` and above: an `AAAA?CC:VV` code with AAAA below
`$8000` parses as a ROM patch but never fires. `is_active` is a bool maintained by
every mutating `CheatSet` method ("any cheat enabled"), so with no cheats
the hot path costs one load and one branch. `read_byte` itself is
untouched. `peek` bypasses cheats on purpose: tools and tests can still
see the real ROM byte.

The first enabled cheat whose address matches, and whose compare byte (if
any) equals the real byte, wins.

### RAM freezes: start of `System::run_frame_with_audio`

`apply_ram_freezes` collects the enabled freezes and pokes each through
`System::poke`, once per frame before the first instruction. Because
`poke` only reaches CPU RAM (`$0000-$1FFF`) and PRG RAM (`$6000-$7FFF`), a
freeze in `$2000-$5FFF` is a silent no-op. Freezing once per frame is what
Game Genie style RAM codes on other emulators do; a value the game rewrites
mid-frame is restored before the next frame's logic runs, which is enough
for counters that only matter at the frame boundary.

## API

```rust
let mut cheat = Cheat::parse("SXIOPO")?;       // enabled, empty description
cheat.description = "Infinite lives".into();
let idx = system.cheats_mut().add(cheat);
system.cheats_mut().toggle(idx);               // returns the new state
system.cheats_mut().remove(idx);
system.cheats().is_active();
system.load_cheats(&path)?;                    // Ok(false) if no file
system.save_cheats(&path)?;
```

`CheatSet` also implements `Display` (the `.cht` text) and `FromStr`.

## `.cht` file format

One cheat per line, tab separated: `CODE<TAB>1|0<TAB>description`. The
enabled flag and description are optional. Blank lines and lines starting
with `#` are ignored. `Display` writes a comment header first.

```text
# nes-emu cheats: CODE<TAB>1|0<TAB>description
SXIOPO	1	Infinite lives
075A:02	0	Freeze lives at 3
```

`load_cheats` replaces the whole set; a malformed line is reported as an
`InvalidData` error naming the line number and leaves the set untouched.
`save_cheats` always writes, even an empty set, so a cleared list does not
come back on the next load.

## Verification

Unit tests in `src/cheat.rs`: `SXIOPO` decodes to `$91D9:$AD`; case and
separator insensitivity; the 8-letter example; 500 random
address/value/compare triples encoded and decoded back; bad letters,
lengths and raw forms rejected; `rom_override` compare and enable
semantics; freezes skip disabled cheats; `.cht` round trip, comment
handling and line-numbered errors.

Unit tests in `src/system/tests.rs` drive the real CPU: `LDA #$11` at the
reset vector with a cheat on `$8001` loads `$22` (and `peek` still shows
`$11`); a compare mismatch leaves `$11`; a disabled cheat leaves `$11`; a
RAM freeze is reapplied at the next `run_frame` after a poke; the file
round trip through `System::save_cheats`/`load_cheats`, including the
missing-file and malformed-file cases.

### Headless Super Mario Bros run (`tests/cheats_smb.rs`, ignored)

```text
cargo test --release --test cheats_smb -- --ignored --nocapture
```

Two runs of the same input script (Start tapped at frame 150, Right held
from frame 300 so Mario walks into the first Goomba), watching `$075A`
(lives minus one, 2 at the start of a game):

```text
control: frame 720: $075A 02 -> 01
control: ran 842 frames, $075A at frame 300 02, final 01
sxiopo: ran 2400 frames, $075A at frame 300 02, final 02
```

Without the cheat the counter drops at frame 720 when the death sequence
finishes. With `SXIOPO`, under identical input, `$075A` stayed `02` for
all 2400 frames. The test measures only the counter, not the deaths
themselves; the identical script and the frame-720 drop in the control
run are what show a death was staged. The test also asserts the real
bytes at `$91D9` are `CE 5A 07` before patching.

Empty-set cost: the existing nestest and blargg suites are unchanged with
the hook in place (see the PR for the run).

## Debugging notes

- A first attempt at the SMB test measured `$075A` from power-on and
  stopped two seconds after the first change. It stopped at frame 153:
  SMB writes `$075A` (0 to 2) at frame 31 during title-screen init, long
  before Start. The trace now starts at the frame Right is pressed.
- `"91D9"` with no colon is parsed as a Game Genie code and fails on the
  letter `9`, not as a malformed raw code. That is intended: anything
  without `:` is treated as Game Genie.
