//! Command registry for the palette.
//!
//! A command is a name the user types, a one-line description and an
//! action: a plain function over the [`App`], a function that also takes
//! the text typed after the name (`cheat add SXIOPO`), or a tool page to
//! open. The built-in set lives in [`builtin_commands`]; tools that need
//! their own commands add them there so registration stays in one place.
//!
//! Matching: while the input has no space it is a case-insensitive
//! subsequence of the name (`fadv` finds `frame advance`). Once the input
//! contains a space the match switches to prefixes, so `cheat add SXIOPO`
//! lists `cheat add` with `SXIOPO` as its argument, while `frame ` still
//! finds `frame advance` on the way to typing it out.

use super::app::App;
use super::tools::ToolId;

#[derive(Clone, Copy)]
pub enum Action {
    /// Run immediately; the palette closes first.
    Run(fn(&mut App)),
    /// Run with the text typed after the command name, trimmed. Empty
    /// when the user typed the bare name.
    RunWithArg(fn(&mut App, &str)),
    /// Replace the palette with the named tool page.
    OpenTool(ToolId),
}

pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
    pub action: Action,
}

impl Command {
    const fn run(name: &'static str, description: &'static str, f: fn(&mut App)) -> Self {
        Command {
            name,
            description,
            action: Action::Run(f),
        }
    }

    const fn run_arg(name: &'static str, description: &'static str, f: fn(&mut App, &str)) -> Self {
        Command {
            name,
            description,
            action: Action::RunWithArg(f),
        }
    }

    const fn tool(name: &'static str, description: &'static str, id: ToolId) -> Self {
        Command {
            name,
            description,
            action: Action::OpenTool(id),
        }
    }
}

/// Width of the name column in the palette and on the Help page. With
/// the default window (44 text columns at font scale 2) that leaves 21
/// columns for the description; longer ones are clipped.
pub const NAME_COLUMNS: usize = 22;

/// Every command the palette offers, in the order they are listed when
/// the filter is empty.
pub fn builtin_commands() -> Vec<Command> {
    vec![
        Command::run("pause", "Stop emulation", App::pause),
        Command::run("resume", "Continue emulation", App::resume),
        Command::run(
            "frame advance",
            "One frame, then pause",
            App::request_frame_advance,
        ),
        Command::run("reset", "Press reset", App::reset),
        Command::run("mute", "Toggle audio mute", App::toggle_mute),
        Command::run("volume up", "Raise volume 10%", App::volume_up),
        Command::run("volume down", "Lower volume 10%", App::volume_down),
        Command::run(
            "toggle overscan crop",
            "Show/hide 8 px border",
            App::toggle_crop,
        ),
        Command::tool("help", "Keys and commands", ToolId::Help),
        Command::run("quit", "Save RAM and exit", App::quit),
        // Cheats (docs/debugging/CHEAT_ENGINE.md, issue #32).
        Command::tool("cheats", "Cheat list page", ToolId::Cheats),
        Command::run_arg("cheat add", "Add code e.g. SXIOPO", App::cheat_add_command),
        Command::run_arg(
            "cheat toggle",
            "Flip cheat number N",
            App::cheat_toggle_command,
        ),
        Command::run("cheat clear", "Delete every cheat", App::clear_cheats),
        // Example tools (issue 33): memory, PPU and APU pages.
        Command::run_arg("mem", "Hex dump; mem ADDR", App::goto_memory),
        Command::tool("ppu", "Patterns/names/pals", ToolId::Ppu),
        Command::tool("apu", "Channel mute page", ToolId::Apu),
        Command::run("mute pulse1", "Silence pulse 1", |app| app.mute_channel(0)),
        Command::run("mute pulse2", "Silence pulse 2", |app| app.mute_channel(1)),
        Command::run("mute triangle", "Silence triangle", |app| {
            app.mute_channel(2)
        }),
        Command::run("mute noise", "Silence noise", |app| app.mute_channel(3)),
        Command::run("mute dmc", "Silence DMC", |app| app.mute_channel(4)),
        Command::run("unmute all", "Restore all channels", App::unmute_all),
        // Save states (docs/debugging/SAVE_STATES.md, issue #39).
        Command::run_arg("save state", "Save to slot N", App::save_state_command),
        Command::run_arg("load state", "Load from slot N", App::load_state_command),
        Command::run_arg("slot", "Pick state slot 1-9", App::slot_command),
        Command::tool("states", "Save state slots page", ToolId::States),
    ]
}

/// True when every character of `needle` appears in `haystack` in order,
/// ignoring case. An empty needle matches everything.
pub fn subsequence_match(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .flat_map(char::to_lowercase)
        .all(|n| hay.any(|h| h == n))
}

/// True when `input` names this command, possibly followed by a space
/// and an argument, or is itself a prefix of the name. Case-insensitive.
/// Used once the input contains a space; see the module docs.
pub fn prefix_match(input: &str, name: &str) -> bool {
    let input = input.to_lowercase();
    let name = name.to_lowercase();
    if name.starts_with(&input) {
        return true;
    }
    match input.strip_prefix(&name) {
        Some(rest) => rest.is_empty() || rest.starts_with(' '),
        None => false,
    }
}

/// The text after the command name in `input`, trimmed. Empty when the
/// input is the bare name or shorter.
pub fn argument(input: &str, name: &str) -> String {
    if input.len() > name.len() && input[..name.len()].eq_ignore_ascii_case(name) {
        input[name.len()..].trim().to_string()
    } else {
        String::new()
    }
}

/// Indices of the commands whose names match `needle`, in registry order.
/// A command with an argument also matches when `needle` is its name
/// followed by the argument text.
pub fn filter(commands: &[Command], needle: &str) -> Vec<usize> {
    let with_arg = needle.contains(' ');
    commands
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            if with_arg {
                prefix_match(needle, c.name)
            } else {
                subsequence_match(needle, c.name)
            }
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_is_case_insensitive_and_ordered() {
        assert!(subsequence_match("", "anything"));
        assert!(subsequence_match("vol", "volume up"));
        assert!(subsequence_match("VU", "volume up"));
        assert!(subsequence_match("fadv", "frame advance"));
        assert!(!subsequence_match("uv", "volume up"));
        assert!(!subsequence_match("volumes", "volume"));
    }

    #[test]
    fn filter_keeps_registry_order() {
        let commands = builtin_commands();
        let all = filter(&commands, "");
        assert_eq!(all.len(), commands.len());
        assert_eq!(all, (0..commands.len()).collect::<Vec<_>>());

        let vol: Vec<&str> = filter(&commands, "vol")
            .into_iter()
            .map(|i| commands[i].name)
            .collect();
        assert_eq!(vol, ["volume up", "volume down"]);

        assert!(filter(&commands, "zzz").is_empty());
    }

    #[test]
    fn space_switches_to_prefix_matching_with_argument() {
        let commands = builtin_commands();
        let names = |needle: &str| -> Vec<&str> {
            filter(&commands, needle)
                .into_iter()
                .map(|i| commands[i].name)
                .collect()
        };
        assert_eq!(names("cheat add SXIOPO"), ["cheat add"]);
        assert_eq!(names("Cheat Add sxiopo"), ["cheat add"]);
        assert_eq!(
            names("cheat "),
            ["cheat add", "cheat toggle", "cheat clear"]
        );
        assert_eq!(names("frame "), ["frame advance"]);
        assert_eq!(names("frame advance"), ["frame advance"]);
        assert!(names("cheat addx").is_empty());
        assert!(names("cheatadd x").is_empty());
        // Without a space the subsequence rule still applies.
        assert_eq!(names("chadd"), ["cheat add"]);
    }

    #[test]
    fn argument_is_the_trimmed_tail() {
        assert_eq!(argument("cheat add SXIOPO", "cheat add"), "SXIOPO");
        assert_eq!(argument("cheat add   075A:02  ", "cheat add"), "075A:02");
        assert_eq!(argument("CHEAT ADD x", "cheat add"), "x");
        assert_eq!(argument("cheat add", "cheat add"), "");
        assert_eq!(argument("cheat", "cheat add"), "");
        assert_eq!(argument("frame ", "frame advance"), "");
    }

    #[test]
    fn builtin_text_fits_the_palette() {
        // 44 columns at the default window size, minus the name column
        // and its separating space.
        const MAX_DESCRIPTION_LEN: usize = 44 - NAME_COLUMNS - 1;
        for c in builtin_commands() {
            assert!(c.name.len() <= NAME_COLUMNS, "{} is too long", c.name);
            assert!(
                c.description.len() <= MAX_DESCRIPTION_LEN,
                "description of {} is too long",
                c.name
            );
        }
    }

    #[test]
    fn argument_commands_match_by_prefix() {
        let commands = builtin_commands();
        assert_eq!(argument("mem 300", "mem"), "300");
        assert_eq!(argument("MEM   c000 ", "mem"), "c000");
        assert_eq!(argument("mem", "mem"), "");
        assert!(prefix_match("mem 300", "mem"));
        assert!(!prefix_match("memory", "mem"));

        let names: Vec<&str> = filter(&commands, "mem 300")
            .into_iter()
            .map(|i| commands[i].name)
            .collect();
        assert_eq!(names, ["mem"]);
    }

    #[test]
    fn builtin_names_are_unique() {
        let commands = builtin_commands();
        for (i, a) in commands.iter().enumerate() {
            for b in &commands[i + 1..] {
                assert_ne!(a.name, b.name);
            }
        }
    }
}
