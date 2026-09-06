# In-Window UI: Font, Command Palette and Tool Pages

**Date:** 2026-09-06
**Type:** Feature / Binary UI
**Status:** Complete; feel of the palette and key conflicts need a human
**Tracking:** GitHub issue #30 (phase 1 of `docs/plans/TOOLS_AND_CHEATS.md`);
argument commands and the Cheats page are issue #32 (phase 3); issue #33
added the Memory, PPU and APU pages

## Executive Summary

The SDL binary had no text rendering and no UI state: every key went
straight to a hotkey or the controller. It now has a small UI layer under
`src/ui/` that draws text with an embedded 8x8 font, opens a command
palette on the backquote key, and shows full-window tool pages (the first
is Help). The emulator library is untouched; everything lives in the
binary. No new crate dependencies.

Two debug flags make the UI checkable without a human at the keyboard:
`--screenshot PATH:N` writes the composed window as PPM on frame N, and
`--ui-script KEYS` injects key presses one per frame from frame 30. Both
were used to produce the screenshots in
`docs/testing/test_output/ui/`.

## Architecture

```
src/main.rs            SDL setup, frame loop, key routing, flags
src/ui/mod.rs          Ui: Game | Palette | Tool state machine, KEY_BINDINGS
src/ui/app.rs          App: System, audio handles, paused/frame_advance/crop/quit
src/ui/font.rs         8x8 font for ASCII 32-126, draw_text, text_width
src/ui/commands.rs     Command { name, description, action }, builtin_commands
src/ui/palette.rs      Palette overlay: input, subsequence filter, selection
src/ui/tool.rs         Tool trait, ToolEvent, page chrome helpers
src/ui/tools/mod.rs    ToolId enum and ToolId::open
src/ui/tools/help.rs   Help page: key bindings and every command
src/ui/tools/cheats.rs Cheats page: list, toggle, add, delete, edit
src/ui/tools/memory.rs Memory page: hex dump of CPU space (issue 33)
src/ui/tools/ppu.rs    PPU page: pattern tables, nametables, palettes
src/ui/tools/apu.rs    APU page: per-channel mute state
```

### Key routing

Every key press goes to `main::key_down`, which calls `Ui::handle_key`
first. In `Game` mode the UI only claims the backquote (opens the
palette) and returns false, so the hotkeys (Escape, F1, P, N, R, M, Plus,
Minus) and the controller mapping run. In `Palette` and `Tool` modes the
UI consumes every press. Key releases always reach the controller so a
direction held when the palette opened does not stick.

Escape closes the palette or tool; in `Game` mode it still quits, as it
always did. Pressing Escape twice from inside the palette therefore exits
the emulator.

### App

`App` (`src/ui/app.rs`) owns the `System`, the audio queue and the
mute/volume handles the audio callback reads, plus `paused`,
`frame_advance`, `crop_enabled`, `crop_dirty`, `quit_requested`,
`osd_until`, `save_path` and the command registry. Commands and tools
mutate it through methods (`pause`, `resume`, `request_frame_advance`,
`reset`, `toggle_mute`, `volume_up`, `volume_down`, `toggle_crop`,
`quit`). The frame loop reads the flags: it skips emulation while paused
(running exactly one frame when `frame_advance` is set), resizes the
window and recreates `src_rect` when `crop_dirty` is set, and leaves the
loop when `quit_requested` is set so the battery flush on exit still
runs. The audio device, texture and canvas stay in `main`.

### Font

`FONT` is `[[u8; 8]; 95]`, built at compile time from `#`/`.` string art
by a `const fn`, bit 7 leftmost. Glyphs use columns 0-6; column 7 is
spacing, row 7 is blank except for descenders. `draw_text` fills one
rectangle per set pixel. The UI draws at `DEFAULT_FONT_SCALE = 2` (16 px
glyphs, 44 text columns on the default 720 px window); the window scale
of 3 would leave only 30 columns.

### Palette

Typing appends the character of the key code (SDL key codes for
printable ASCII equal the character), Backspace deletes, Up/Down move,
Enter runs the selected match and closes, Escape or backquote closes.
Matching is `commands::subsequence_match`: case-insensitive, every
character of the input must appear in the name in order. Up to eight
matches are shown; the list scrolls to keep the selection visible. The
name column is `commands::NAME_COLUMNS` (22) wide, leaving 21 columns
for the description at the default window size; a unit test keeps the
built-in descriptions inside that.

### Tools

```rust
pub trait Tool {
    fn title(&self) -> &str;
    fn handle_key(&mut self, key: Keycode, app: &mut App) -> ToolEvent;
    fn draw(&self, canvas: &mut WindowCanvas, font_scale: u32, app: &App) -> Result<(), String>;
    fn tick(&mut self, _app: &mut App) {}
}
```

`Ui` draws the backdrop and title bar (`tool::draw_frame`), then calls
`draw` for the body, which starts at `tool::body_top(font_scale)`.
`tool::body_grid` returns the columns and rows that fit, `tool::clip`
truncates a line, `tool::line_step` is the row pitch (glyph plus two
font pixels of leading). Escape closes the tool before it sees the key,
unless the tool's `captures_escape` (default false) returns true; the
Cheats page does that while an inline text entry is open so Escape
cancels the entry instead of the page. Returning `ToolEvent::Close`
closes the tool from any other key. `tick` runs once per presented frame
while the tool is open, paused or not.

## Adding a tool

1. Create `src/ui/tools/<name>.rs` with a struct implementing `Tool`.
2. Add a variant to `ToolId` in `src/ui/tools/mod.rs` and construct the
   tool in `ToolId::open`.
3. Add `Command::tool("<name>", "<description>", ToolId::<Name>)` to
   `builtin_commands` in `src/ui/commands.rs`.

## Adding a command

Add a line to `builtin_commands`: `Command::run("name", "description",
App::method)` for an immediate action, or `Command::tool` to open a page.
`Action::Run` takes a plain `fn(&mut App)`, so the action is usually a
method on `App` (a non-capturing closure such as
`|app| app.mute_channel(0)` coerces to one too); add a method there if
the state it needs is not exposed yet. Keep the name at most 22
characters and the description at most 21, or the palette clips it at
the default window size (a unit test checks the built-ins).

### Commands with an argument

`Command::with_arg("mem", "Hex dump; mem ADDR", App::goto_memory)` makes
an `Action::RunWithArg(fn(&mut App, &str))`. Such a command matches the
palette input in two ways: the usual subsequence match on its name, and
a prefix match when the input is the name followed by whitespace and
the argument (`commands::argument` returns `Some("300")` for `mem 300`,
`Some("")` for `mem`, `None` for `memory`). On Enter the palette passes
the trimmed argument through `PaletteEvent::Run(index, arg)` to
`Ui::run_command`, which calls the function with it.

The function only sees `App`, and only `Ui` can open a page, so a command
that wants to end on a tool sets `app.pending_tool = Some(ToolId::...)`;
`run_command` takes it after any action and opens that page. That is how
`mem ADDR` sets `App::memory_addr` and then opens Memory.

The palette maps key codes to characters and ignores Shift, so an
argument cannot contain `$`; hex addresses are typed bare or with `0x`
(`App::parse_hex_address` also accepts `$` for callers that build the
string themselves).

## Example pages (issue 33)

Three pages exercise the framework and give the palette something to
do. Each reads the emulator state live on every draw, so they follow
the game while it runs (and hold still while paused). All three close on
Escape or Q.

### Memory

`mem` opens a hex dump of CPU space; `mem ADDR` opens it at a hex
address (`mem 300`, `mem 0xc000`; the row is aligned to 16 bytes). Rows
are `System::peek`: sixteen bytes with a gap after eight and an ASCII
column, `.` for anything outside 0x20-0x7E. `peek` refuses to touch the
register range, so rows in `$2000-$5FFF` read as zero and are drawn
dimmed. The header shows the address range on screen. Up/Down move a
row, PageUp/PageDown a page, Home and End go to `$0000` and the last
full page; the first address lives in `App::memory_addr` and survives
closing the page.

A row is 72 characters, more than the 44 columns the window offers at
the UI font scale, so the dump is drawn at `font_scale / 2` (8 px
glyphs on the default window, 60 rows = 960 bytes per page) while the
header keeps the normal size. The page size is remembered from the last
draw in a `Cell` so the paging keys know it.

![Memory page](../testing/test_output/ui/memory.png)

### PPU

`ppu` opens a page with three views; Left/Right (or Tab) switch between
them and the header shows which is active.

- Patterns: both pattern tables at 2x (128x128 tiles each), read through
  `System::peek_chr` so CHR bank switching shows immediately, coloured
  with one of the eight palettes: Up/Down cycle, 0-7 pick directly. The
  info line shows which table PPUCTRL currently assigns to the
  background and to sprites, and the sprite size.
- Nametables: the four nametables as a 2x2 grid at half scale (each 8x8
  tile becomes 4x4 by sampling every other pixel), resolved through the
  cartridge's current mirroring (`$2400 -> 1` means the second logical
  table is physical table 1), coloured through the attribute bytes and
  drawn from the background pattern table PPUCTRL selects. Mirroring
  reads `cartridge.mapper.mirroring()`; the view re-derives the
  logical-to-physical table mapping since the PPU's own helper is
  private.
- Palettes: the 32 palette RAM entries as swatches with their hex value,
  four background rows and four sprite rows. Entry 0 of BG1-BG3 shows
  the universal background colour, as the PPU renders it.

Everything is built into a small RGB image and drawn as horizontal runs
of one colour with `fill_rect`, which is a few thousand rectangles per
frame: slow by rendering standards, far under a frame in practice.

![Pattern tables](../testing/test_output/ui/ppu_patterns.png)
![Nametables](../testing/test_output/ui/ppu_nametables.png)
![Palettes](../testing/test_output/ui/ppu_palettes.png)

### APU

`apu` lists the five mixer channels (pulse1, pulse2, triangle, noise,
dmc) with `playing` or `muted`; keys 1-5 toggle one, U unmutes all. The
state is the library's per-channel mute flags, so the palette commands
`mute pulse1` .. `mute dmc` and `unmute all` change what the page shows.
Master mute (M) is shown separately and is the existing audio-callback
flag, not an APU state.

![APU page](../testing/test_output/ui/apu.png)

### Library accessors added for the pages

The emulator library stays UI-free; the pages needed three small,
read-only additions:

- `Mapper::ppu_peek(&self, addr) -> u8`, a default trait method (open
  bus) overridden by every mapper with a copy of its `ppu_read` body:
  the same CHR byte through the current banking, without clocking
  anything. `System::peek_chr(addr)` calls it for `addr & 0x1FFF` and
  returns 0 with no cartridge. Tests check `ppu_peek == ppu_read` after
  bank switches on mappers 1, 3, 4 and 5, CHR RAM writes on mapper 0,
  and that `peek_chr` leaves `ppu.scanline`/`ppu.cycle` untouched.
- `Apu::set_channel_muted(ch, muted)` / `Apu::channel_muted(ch)`, indexed
  like `apu::CHANNEL_NAMES` (`CHANNEL_COUNT` is 5). The flags gate the
  channel's level inside `get_output` only, so the channel keeps
  running and unmuting is seamless; out-of-range indices are ignored.
  Tests check that muting pulse 1 silences the mix while its own output
  is unchanged, and that muting another channel does not.
- `ppu::NES_PALETTE` is now `pub` so the binary can turn palette RAM
  entries into RGB without a second copy of the 64 colours.

## Argument commands

`Action::RunWithArg(fn(&mut App, &str))` is a command that receives the
text typed after its name, trimmed: `cheat add SXIOPO` runs
`App::cheat_add_command(app, "SXIOPO")`. Register one with
`Command::run_arg`. Commands that take no argument ignore any trailing
text.

The palette filter changes rule once the input contains a space
(`commands::filter`):

- No space: case-insensitive subsequence match, as before (`fadv` finds
  `frame advance`).
- With a space: `commands::prefix_match`. A command matches when its
  name starts with the input (`frame ` still lists `frame advance` while
  it is being typed), or the input starts with the name followed by the
  end of the input or a space (`cheat add SXIOPO` lists exactly
  `cheat add`, not `cheats`). `cheat ` lists the three `cheat ...`
  commands.

On Enter the palette computes the argument with `commands::argument`
(the input past the name, trimmed) and reports
`PaletteEvent::Run(index, arg)`; `Ui::run_command(index, arg, app)`
dispatches on the action. The palette types characters from key codes,
so Shift is ignored: the screenshot shows `cheat add sxiopo` in lower
case, and codes parse case-insensitively anyway. The two shifted
characters raw codes need are mapped by `App::add_cheat`: `;` becomes
`:` and `/` becomes `?`, so `075A;02` on the keyboard is `075A:02`.

Argument commands report failures through `App::cheat_error`, which the
Cheats page shows in red the next time it is open (the palette closes on
Enter, so there is nowhere else to show them) and the log at warn level.

## Cheats page

`src/ui/tools/cheats.rs`, opened by the `cheats` command. Header with the
count, the enabled count and the `.cht` file name; one row per cheat:
1-based number (what `cheat toggle N` takes), `[x]` or `[ ]`, the code
and the description. The list scrolls to keep the cursor visible when
there are more cheats than rows.

| Key | Action |
| --- | --- |
| Up / Down, PageUp / PageDown, Home / End | Move the cursor |
| Space or Return | Toggle the selected cheat |
| D, Delete or Backspace | Delete the selected cheat |
| A or Insert | Add: type the code, Enter, type the description, Enter |
| E | Edit the selected description |
| Escape | Cancel the open text entry; otherwise close the page |
| Q | Close the page |

The page keeps a `Mode`: `List`, `EnterCode`, `EnterDescription` (the
code already parsed) or `EditDescription`. In the entry modes every
printable key types, so `a`, `d` and `e` are letters, not commands. The
code is parsed on the first Enter; a rejected code stays in the entry
with the error shown in red under the list, and the description is only
asked for once the code is valid. Every change goes through
`App::add_cheat`, `toggle_cheat`, `remove_cheat`,
`set_cheat_description` or `clear_cheats`, which rewrite the `.cht`
file (see `docs/debugging/CHEAT_ENGINE.md`).

Palette commands: `cheats` opens the page, `cheat add CODE [description]`
adds and enables a cheat, `cheat toggle N` flips the cheat numbered N on
the page, `cheat clear` deletes all of them.

Screenshots in `docs/testing/test_output/ui/`:

- `cheats.png`: the page after adding `075A:02` through the A entry and
  toggling it off with Space (the `.cht` on disk had `SXIOPO` before the
  run).
- `cheats-reloaded.png`: a second run of the emulator, same ROM, page
  opened from the palette; both cheats came back from the file with the
  second still disabled.
- `cheats-error.png`: the entry after typing `zzz` and Enter: the error
  in red, the entry still open.
- `palette-cheat-add.png`: the palette with `cheat add sxiopo` typed and
  only `cheat add` listed.

Made with (`smb.nes` is a copy of Super Mario Bros. in a scratch
directory, so the `.cht` lands there):

```sh
printf 'SXIOPO\t1\tInfinite lives\n' > smb.cht
./target/debug/nes-emu smb.nes --no-audio \
  --ui-script "backquote,c,h,e,a,t,s,Return,a,0,7,5,a,;,0,2,Return,l,i,v,e,s,Return,Space,,,Escape,Escape" \
  --screenshot cheats.ppm:55
./target/debug/nes-emu smb.nes --no-audio \
  --ui-script "backquote,c,h,e,a,t,Space,a,d,d,Space,s,x,i,o,p,o,,,Escape,backquote,c,h,e,a,t,s,Return,,,Escape,backquote,c,h,e,a,t,Space,t,o,g,g,l,e,Space,2,Return,backquote,c,h,e,a,t,s,Return,a,z,z,z,Return,,,Escape,Escape,Escape" \
  --screenshot palette-cheat-add.ppm:48 --screenshot cheats-reloaded.ppm:59 \
  --screenshot cheats-error.ppm:91
```

The second run also runs `cheat toggle 2` from the palette between the
two page screenshots; the file afterwards reads `075A:02<TAB>1<TAB>lives`.

## Key bindings

| Key | Action |
| --- | --- |
| Backquote | Open the command palette |
| F1 | Open the Help page |
| Escape | Close the palette or tool; in the game, quit |
| P | Pause / resume |
| N | Frame advance (pauses first) |
| R | Reset |
| M | Mute / unmute |
| Plus / Minus | Volume up / down |
| F5 / F8 | Save / load the current save-state slot (docs/debugging/SAVE_STATES.md) |
| F6 / F7 | Previous / next save-state slot |
| Z / X | A / B |
| Right Shift | Select |
| Return | Start |
| Arrows | D-pad |

Inside the palette: type to filter, Backspace, Up/Down, Enter, Escape.
Inside Help: Up/Down/PageUp/PageDown/Home scroll; Escape, Q or Return
close. Inside Cheats: see the table above.
Inside Memory: Up/Down row, PageUp/PageDown page, Home/End; Escape or Q
close.
Inside PPU: Left/Right (Tab) switch view, Up/Down or 0-7 pick the
pattern palette; Escape or Q close.
Inside APU: 1-5 toggle a channel, U unmutes all; Escape or Q close.
Inside States: Up/Down or 1-9 pick a slot, Return loads, S saves;
Escape or Q close (issue #39, docs/debugging/SAVE_STATES.md).

Built-in commands: pause, resume, frame advance, reset, mute, volume up,
volume down, toggle overscan crop, help, quit, cheats, cheat add CODE,
cheat toggle N, cheat clear, mem [ADDR], ppu, apu, mute pulse1, mute
pulse2, mute triangle, mute noise, mute dmc, unmute all, save state [N],
load state [N], slot [N], states.

## Debug flags

### `--screenshot PATH:N`

On the frame where N frames have already been presented (paused frames
count), the composed back buffer is read with `canvas.read_pixels` and
written to PATH as binary PPM (`P6`, window size, which is 720x672 with
the default crop). The read happens before `present`, because after
present the back buffer is undefined on Metal. The flag may be repeated.
The path is split from N at the last colon.

### `--ui-script KEYS`

A comma-separated list of SDL key names (`Keycode::from_name`), with the
aliases `backquote`/`grave`, `enter` and `esc`. Starting on presented
frame 30, one entry per frame is pressed and released through the same
`key_down`/`key_up` functions real events use; an empty entry is a frame
with no key. This is in-process injection into the emulator's own event
handling, not OS-level keystroke injection: it never types into another
window.

The screenshots in `docs/testing/test_output/ui/` were made with:

```sh
./target/debug/nes-emu roms/SuperMarioBros.nes --no-audio \
  --ui-script "backquote,v,o,l,,,Escape,backquote,h,e,l,p,Return,,,Escape,Escape" \
  --screenshot palette.ppm:35 --screenshot help.ppm:44
```

Frames 30-33 open the palette and type `vol`; frame 35 is captured with
the two volume commands listed and the first selected. Frame 36 closes
the palette, 37-42 reopen it, type `help` and press Enter, which opens
the Help page; frame 44 is captured. The final two Escapes close Help and
quit, so the run ends by itself. Convert PPM to PNG with the stdlib
Python recipe in `docs/testing/COMPATIBILITY_SWEEP.md`, reading the size
from the header instead of assuming 256x240.

The issue 33 pages were captured from `roms/mario.nes` the same way,
with 90 empty entries first so the title screen is up, then:

```
backquote,m,e,m,Space,3,0,0,Return     memory.png at +4, PageDown,
                                       memory_page2.png at +4, Escape
backquote,p,p,u,Return                 ppu_patterns.png at +4
Right                                  ppu_nametables.png at +4
Right                                  ppu_palettes.png at +4, Escape
backquote,a,p,u,Return,2,5             apu.png at +4 (pulse2, dmc muted)
Escape,backquote,u,n,m,u,t,e,Return    unmute all
backquote,a,p,u,Return                 apu_unmuted.png at +4
Escape,Escape                          close, quit
```

`+4` is the frame number of the last key plus four (two frames for the
page to draw, captured on the third). The key names are SDL names:
`Space`, `PageDown`, `Right`, digits as `2`.

## Verification

- `cargo test`: unit tests for the font table (95 glyphs, MSB-leftmost
  packing, column 7 clear), the subsequence filter, the registry (unique
  names, text fits the palette) and the palette state machine (typing,
  selection clamping, Enter/Escape). None open a window.
- Screenshots: `docs/testing/test_output/ui/palette.png` (palette with
  `vol` typed) and `docs/testing/test_output/ui/help.png` (Help page).
- Issue 33: unit tests for `commands::argument` and the prefix filter,
  the palette passing `mem c000` through, `parse_hex_address`, the
  memory row format and last-page clamp, the PPU view cycle, the
  mirroring table map and pattern/palette lookups against a synthetic
  CHR RAM cartridge, and the APU digit-key map. Screenshots
  `memory.png`, `memory_page2.png`, `ppu_patterns.png`,
  `ppu_nametables.png`, `ppu_palettes.png`, `apu.png` and
  `apu_unmuted.png` in the same directory, all taken while
  `roms/mario.nes` sat on its title screen.
- The scripted run above exercises open, filter, run (help), close and
  quit end to end, and the emulator exits through the normal path so the
  battery flush runs.

## Debugging trail

The first screenshots showed two layout problems that unit tests could
not catch:

1. Help rows touched: with no leading, descenders (g, p, y) ran into the
   next line. Fixed with `tool::line_step` (glyph height plus two font
   pixels), used by `body_grid` and the Help page.
2. Descriptions clipped: at 44 columns a 22-column name plus a two-space
   indent left 19 characters, so "Stop emulation, keep the last frame"
   became "Stop emulation, kee" in both the palette and Help. Fixed by
   dropping the indent on Help, sharing `NAME_COLUMNS` between the two,
   shortening the built-in descriptions to 21 characters and adding
   `builtin_text_fits_the_palette` so new commands cannot regress it.

Earlier, `Keycode::into_i32` does not exist in sdl2 0.36; the key code is
`#[repr(i32)]`, so `key as i32` is the conversion the palette uses.

Cheats page (issue #32):

1. The first footer hint, `Space toggle  A add  D delete  E edit  Esc
   close`, is 45 characters and lost its last word at 44 columns;
   shortened to `Space toggle A add D del E edit Esc close`.
2. The palette draws 43 columns inside its panel (window width minus
   the panel and text padding), one fewer than the Help page and the
   `builtin_text_fits_the_palette` test assume, so a 21-character
   description such as `Add code, e.g. SXIOPO` ran into the panel edge.
   The description was shortened to 20 characters; the off-by-one in the
   framework is left alone (it only shows at the maximum length).
3. A `--screenshot` frame that coincides with a scripted key captures the
   frame after that key: keys are injected before the draw. The third
   screenshot of the first attempt landed on the Escape that closed the
   page and showed the game.
4. `Ui` closed a tool on Escape before the tool saw the key, so an
   Escape meant to cancel the text entry closed the page; hence
   `Tool::captures_escape`.
5. Key releases reach the controller in every mode, so the Return that
   confirms an entry logs a `START released` with nothing pressed. This
   was already the case for Enter in the palette and is harmless.
Issue 33 pages:

3. The PPU page's tab line `[Patterns]  Nametables   Palettes   Left/Right`
   was 46 columns and the first screenshot showed `Left/Righ`. The
   inactive tabs lost their padding spaces (42 columns now).
4. A 16-byte hex row with ASCII is 72 columns, impossible at the UI
   font scale; rather than drop to 8 bytes per row the dump body is
   drawn at half the font scale, which is what the memory screenshots
   show.
5. `fn(&mut App)` cannot open a page, so `mem ADDR` needed the
   `pending_tool` hand-off described above rather than a special-cased
   action.

## Needs a human

- Issue 33: readability of the 8 px memory dump on a real display, the
  2x pattern tables and half-scale nametables at other window scales,
  and whether the APU mute should also show on the volume OSD.
- Feel of the palette: panel size, colours, the block cursor, whether
  eight rows is enough.
- Key conflicts: P, N and F1 are new hotkeys; Escape from the palette
  followed by another Escape quits. Typing in the palette never reaches
  the controller, but a Z or X held when the palette opens is released on
  key-up as usual.
- Pause with audio on: the callback repeats the last sample while the
  queue is empty, which is silent in practice but not a hard mute.
- Cheats page: typing on a real keyboard (the scripted runs press key
  codes, so Shift and the `;` to `:` mapping have only been exercised
  that way), and whether Backspace doubling as delete in the list is a
  surprise.
