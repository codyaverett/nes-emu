pub mod apu;
pub mod cartridge;
pub mod cheat;
pub mod input;
pub mod ppu;
pub mod state;
pub mod system;
/// In-window overlay UI shared by the SDL binary and the web page:
/// command palette, tool pages, toasts and rewind
/// (docs/plans/SHARED_OVERLAY_UI.md).
pub mod ui;
