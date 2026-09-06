# In-Window UI: Font, Command Palette and Tool Pages

**Date:** 2026-09-06
**Type:** Feature / Binary UI
**Status:** Complete; feel of the palette and key conflicts need a human
**Tracking:** GitHub issue #30 (phase 1 of `docs/plans/TOOLS_AND_CHEATS.md`)

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
font pixels of leading). Escape never reaches a tool; returning
`ToolEvent::Close` closes it from any other key. `tick` runs once per
presented frame while the tool is open, paused or not.

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
method on `App`; add one there if the state it needs is not exposed yet.
Keep the name at most 22 characters and the description at most 21, or
the palette clips it at the default window size (a unit test checks the
built-ins).

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
| Z / X | A / B |
| Right Shift | Select |
| Return | Start |
| Arrows | D-pad |

Inside the palette: type to filter, Backspace, Up/Down, Enter, Escape.
Inside Help: Up/Down/PageUp/PageDown/Home scroll; Escape, Q or Return
close.

Built-in commands: pause, resume, frame advance, reset, mute, volume up,
volume down, toggle overscan crop, help, quit.

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

## Verification

- `cargo test`: unit tests for the font table (95 glyphs, MSB-leftmost
  packing, column 7 clear), the subsequence filter, the registry (unique
  names, text fits the palette) and the palette state machine (typing,
  selection clamping, Enter/Escape). None open a window.
- Screenshots: `docs/testing/test_output/ui/palette.png` (palette with
  `vol` typed) and `docs/testing/test_output/ui/help.png` (Help page).
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

## Needs a human

- Feel of the palette: panel size, colours, the block cursor, whether
  eight rows is enough.
- Key conflicts: P, N and F1 are new hotkeys; Escape from the palette
  followed by another Escape quits. Typing in the palette never reaches
  the controller, but a Z or X held when the palette opens is released on
  key-up as usual.
- Pause with audio on: the callback repeats the last sample while the
  queue is empty, which is silent in practice but not a hard mute.
