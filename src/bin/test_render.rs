use nes_emu::cartridge::Cartridge;
use nes_emu::system::System;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <rom_file>", args[0]);
        std::process::exit(1);
    }

    let rom_path = &args[1];
    println!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;
    println!("ROM loaded. Mapper: {}", cartridge.mapper);

    let mut system = System::new();

    // Enable PPU debug
    system.ppu.set_debug_flags(true, true, false, false);

    system.load_cartridge(cartridge);

    println!("\n=== Running for 120 frames ===\n");

    for frame_num in 0..120 {
        // Run one frame
        system.run_frame_with_audio(None);

        // Save specific frames
        if frame_num == 0 || frame_num == 30 || frame_num == 60 || frame_num == 119 {
            println!("--- Frame {} ---", frame_num);

            // Save frame
            let filename = format!("test_frame_{}.ppm", frame_num);
            system.ppu.save_frame_to_ppm(&filename)?;
            println!("Saved: {}", filename);

            // Save debug info
            let debug_filename = format!("test_frame_{}_debug.txt", frame_num);
            system.ppu.save_frame_debug_info(&debug_filename)?;
            println!("Saved: {}", debug_filename);
            println!("MASK: 0x{:02X}, CTRL: 0x{:02X}",
                system.ppu.mask.bits(), system.ppu.ctrl.bits());
        }

        // Log when rendering gets enabled
        static mut RENDERING_ENABLED: bool = false;
        unsafe {
            let rendering_now = system.ppu.mask.bits() & 0x18 != 0;
            if rendering_now && !RENDERING_ENABLED {
                println!(">>> Rendering ENABLED at frame {}! MASK=0x{:02X}",
                    frame_num, system.ppu.mask.bits());
                RENDERING_ENABLED = true;
            }
        }
    }

    println!("\n=== Final PPU State ===");
    println!("Frame: {}", system.ppu.frame);
    println!("Scanline: {}", system.ppu.scanline);
    println!("Cycle: {}", system.ppu.cycle);
    println!("CTRL: 0x{:02X}", system.ppu.ctrl.bits());
    println!("MASK: 0x{:02X}", system.ppu.mask.bits());
    println!("STATUS: 0x{:02X}", system.ppu.status.bits());

    Ok(())
}
