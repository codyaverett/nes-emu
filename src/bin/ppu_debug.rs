use nes_emu::cartridge::Cartridge;
use nes_emu::ppu::Ppu;
use nes_emu::system::System;
use std::cell::RefCell;
use std::rc::Rc;

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let rom_path = "./roms/mario.nes";
    println!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;
    println!("ROM loaded successfully");
    println!("  Mapper: {}", cartridge.mapper);
    println!("  PRG ROM: {} KB", cartridge.prg_rom.len() / 1024);
    println!("  CHR ROM: {} KB", cartridge.chr_rom.len() / 1024);
    println!("  CHR RAM: {} KB", cartridge.chr_ram.len() / 1024);

    // Test CHR ROM reads
    println!("\nTesting CHR ROM reads:");
    for addr in &[0x0000, 0x0010, 0x0100, 0x1000, 0x1010] {
        let value = cartridge.read_chr(*addr);
        println!("  CHR[0x{:04X}] = 0x{:02X}", addr, value);
    }

    // Create a minimal system to test PPU
    println!("\nCreating system...");
    let mut system = System::new();
    system.load_cartridge(cartridge);
    system.reset();

    println!("\nRunning 10 frames to let game initialize...");
    for frame in 0..10 {
        system.run_frame_with_audio(None);
        if frame == 0 || frame == 9 {
            println!("Frame {}: CTRL=0x{:02X}, MASK=0x{:02X}, STATUS=0x{:02X}",
                frame,
                system.ppu.ctrl.bits(),
                system.ppu.mask.bits(),
                system.ppu.status.bits());
        }
    }

    // Check PPU state
    println!("\nPPU State after 10 frames:");
    println!("  v (VRAM addr): 0x{:04X}", system.ppu.v);
    println!("  t (temp addr): 0x{:04X}", system.ppu.t);
    println!("  x (fine X):    {}", system.ppu.x);
    println!("  CTRL: 0x{:02X}", system.ppu.ctrl.bits());
    println!("  MASK: 0x{:02X}", system.ppu.mask.bits());

    // Check first few palette entries
    println!("\nPalette (first 16 entries):");
    for i in 0..16 {
        print!("  [${:02X}]=0x{:02X}", i, system.ppu.palette[i]);
        if (i + 1) % 4 == 0 {
            println!();
        }
    }

    // Check nametable 0 (first 64 bytes)
    println!("\nNametable 0 (first 64 bytes):");
    for row in 0..4 {
        print!("  ");
        for col in 0..16 {
            let idx = row * 16 + col;
            print!("{:02X} ", system.ppu.vram[idx]);
        }
        println!();
    }

    Ok(())
}
