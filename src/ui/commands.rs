//! Command registry for the palette.
//!
//! A command is a name the user types, a one-line description and an
//! action: either a plain function over the [`App`] or a tool page to
//! open. The built-in set lives in [`builtin_commands`]; tools that need
//! their own commands add them there so registration stays in one place.

use super::app::App;
use super::tools::ToolId;

#[derive(Clone, Copy)]
pub enum Action {
    /// Run immediately; the palette closes first.
    Run(fn(&mut App)),
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

/// Indices of the commands whose names match `needle`, in registry order.
pub fn filter(commands: &[Command], needle: &str) -> Vec<usize> {
    commands
        .iter()
        .enumerate()
        .filter(|(_, c)| subsequence_match(needle, c.name))
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
    fn builtin_names_are_unique() {
        let commands = builtin_commands();
        for (i, a) in commands.iter().enumerate() {
            for b in &commands[i + 1..] {
                assert_ne!(a.name, b.name);
            }
        }
    }
}
