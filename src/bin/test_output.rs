use std::env;
use nes_emu::cartridge::Cartridge;
use nes_emu::system::System;

fn main() {
    
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <rom_file>", args[0]);
        std::process::exit(1);
    }
    
    let rom_path = &args[1];
    println!("Loading ROM: {}", rom_path);
    
    let cartridge = Cartridge::load_from_file(rom_path).unwrap();
    println!("ROM loaded. Mapper: {}", cartridge.mapper);
    
    let mut system = System::new();
    system.load_cartridge(cartridge);
    
    // Run a few frames
    for frame in 0..60 {
        system.run_frame();
        
        if frame % 20 != 0 {
            continue;
        }
        
        // Check if frame buffer has any non-zero pixels
        let buffer = system.get_frame_buffer();
        let non_zero_pixels = buffer.iter().filter(|&&x| x != 0).count();
        
        // Check PPU registers
        let ppu = &system.ppu;
        println!("Frame {}: {} non-zero pixels out of {} | PPU ctrl: 0x{:02X}, mask: 0x{:02X}", 
                 frame, non_zero_pixels, buffer.len(), ppu.ctrl.bits(), ppu.mask.bits());
        
        // Show first few unique colors
        let mut colors = Vec::new();
        for i in (0..buffer.len()).step_by(3) {
            let color = (buffer[i], buffer[i+1], buffer[i+2]);
            if !colors.contains(&color) {
                colors.push(color);
                if colors.len() >= 5 {
                    break;
                }
            }
        }
        println!("  First few colors: {:?}", colors);
    }
}