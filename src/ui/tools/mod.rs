//! One file per tool page. Register a new tool here: add a variant to
//! [`ToolId`], construct it in [`ToolId::open`], and add a
//! `Command::tool` line in `commands::builtin_commands`.

pub mod help;

use super::tool::Tool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolId {
    Help,
}

impl ToolId {
    pub fn open(self) -> Box<dyn Tool> {
        match self {
            ToolId::Help => Box::new(help::Help::default()),
        }
    }
}
