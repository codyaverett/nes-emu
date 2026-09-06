# Command Palette, Tool Pages and Cheats

**Date:** 2026-09-06
**Type:** Feature Plan
**Status:** Issues filed; implementation in lanes
**Tracking:** GitHub issues listed per phase below

## Goal

Give the SDL binary an extensible in-app tool system and a cheat engine:

- A command palette (one key opens it, type to filter, Enter runs) that lists
  every registered command. Commands can act immediately or open a tool page.
- Tool pages: full-screen overlays drawn on top of, or instead of, the game
  view, with their own key handling. Adding a tool is one new file that
  implements a trait and one registration line.
- Cheats: Game Genie codes and raw address/value freezes, toggled per cheat,
  persisted per ROM, editable from a tool page and from palette commands.

## Constraints

- No new crate dependencies. The binary has no text rendering today; the
  overlay uses an 8x8 bitmap font embedded in code (ASCII 32-126).
- The emulator library stays UI-free. Cheats live in the library (they change
  bus reads); everything visual lives in the binary under `src/ui/`.
- The library's test ROM harness must stay green; cheat hooks in the bus path
  cost nothing when no cheat is active.

## Architecture

```
src/ui/mod.rs        Ui state machine: Game | Palette | Tool(id); key routing
src/ui/font.rs       8x8 bitmap font, draw_text(canvas, x, y, scale, text)
src/ui/palette.rs    Command palette overlay: input line, filtered list, cursor
src/ui/commands.rs   Command registry: name, description, Action enum
src/ui/tool.rs       trait Tool { fn title, fn handle_key, fn draw, fn tick }
src/ui/tools/        One file per tool page
src/cheat.rs         library: Cheat, CheatSet, Game Genie decode, apply()
```

Command actions: `Run(fn(&mut App))`, `OpenTool(ToolId)`, `Toggle(...)`.
The registry is a `Vec<Command>` built at startup; tools push their own
commands so registration stays in one place per tool.

Bus hook: `System::read_byte` for `$8000-$FFFF` consults `CheatSet` only when
it is non-empty (a bool checked first). Game Genie semantics: if the address
matches and (no compare byte, or the ROM byte equals the compare byte) the
cheat value is returned. RAM freezes are applied once per frame in
`run_frame` by poking the value.

## Phases

### Phase 1: overlay text, palette framework, tool pages (#30)
Font, `Ui` state machine, palette with fuzzy filter, `Tool` trait, and
built-in commands: pause, frame advance, reset, mute, volume up/down, toggle
overscan crop, quit, and a "Help" tool page that lists key bindings. Palette
opens with the backquote key; Escape closes palette or tool.

### Phase 2: cheat engine in the library (#31)
`src/cheat.rs` with Game Genie 6 and 8 letter decoding, raw codes
(`AAAA:VV` and `AAAA?CC:VV`), enable flags, `.cht` file next to the ROM
(one cheat per line: code, description, enabled), and the bus hook. Unit
tests against the published Game Genie decode examples.

### Phase 3: cheat tool page and commands (#32)
Tool page listing cheats with toggle, add, delete, and description editing;
palette commands `cheat add <code>`, `cheat toggle <n>`, `cheat clear`.
Persist on change.

### Phase 4: example tools (#33)
Memory viewer (hex dump with paging and a goto command), PPU viewer (pattern
tables, nametables, palettes), APU channel mute toggles. These prove the
framework and are what the palette is for.

## Verification

- Library: unit tests for decoding and application; a blargg run with an
  empty cheat set is unchanged.
- Binary: the UI has no headless harness. Each tool renders into the SDL
  canvas only; a `--screenshot <path>` debug flag writing the composed frame
  as PPM after N frames is added in Phase 1 so tool pages can be checked
  without a human, and used in the PRs.
