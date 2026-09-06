//! One file per tool page. Register a new tool here: add a variant to
//! [`ToolId`], construct it in [`ToolId::open`], and add a
//! `Command::tool` line in `commands::builtin_commands`.

pub mod help;
// Cheats (issue #32).
pub mod cheats;
// Example tools (issue 33).
pub mod apu;
pub mod memory;
pub mod ppu;

use super::tool::Tool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolId {
    Help,
    // Cheats (issue #32).
    Cheats,
    // Example tools (issue 33).
    Memory,
    Ppu,
    Apu,
}

impl ToolId {
    pub fn open(self) -> Box<dyn Tool> {
        match self {
            ToolId::Help => Box::new(help::Help::default()),
            // Cheats (issue #32).
            ToolId::Cheats => Box::new(cheats::Cheats::default()),
            // Example tools (issue 33).
            ToolId::Memory => Box::new(memory::Memory::default()),
            ToolId::Ppu => Box::new(ppu::PpuView::default()),
            ToolId::Apu => Box::new(apu::ApuView),
        }
    }
}
