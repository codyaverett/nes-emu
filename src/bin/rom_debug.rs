use nes_emu::cartridge::Cartridge;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <rom_file>", args[0]);
        std::process::exit(1);
    }

    let rom_path = &args[1];
    println!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;

    println!("\n=== ROM Information ===");
    println!("Mapper: {}", cartridge.mapper);
    println!("Mirroring: {:?}", cartridge.get_mirroring());
    println!("PRG ROM size: {} bytes ({} KB)",
        cartridge.prg_rom.len(), cartridge.prg_rom.len() / 1024);
    println!("CHR ROM size: {} bytes ({} KB)",
        cartridge.chr_rom.len(), cartridge.chr_rom.len() / 1024);
    println!("Has CHR RAM: {}", cartridge.chr_ram.len() > 0);
    if cartridge.chr_ram.len() > 0 {
        println!("CHR RAM size: {} bytes", cartridge.chr_ram.len());
    }

    println!("\n=== First 16 bytes of PRG ROM ===");
    for (i, byte) in cartridge.prg_rom.iter().take(16).enumerate() {
        if i % 8 == 0 && i > 0 {
            println!();
        }
        print!("{:02X} ", byte);
    }
    println!();

    println!("\n=== First 64 bytes of CHR ROM/RAM ===");
    if cartridge.chr_rom.len() > 0 {
        for (i, byte) in cartridge.chr_rom.iter().take(64).enumerate() {
            if i % 16 == 0 && i > 0 {
                println!();
            }
            print!("{:02X} ", byte);
        }
    } else if cartridge.chr_ram.len() > 0 {
        println!("(Using CHR RAM - initially all zeros)");
        for i in 0..64 {
            if i % 16 == 0 && i > 0 {
                println!();
            }
            print!("{:02X} ", cartridge.chr_ram[i]);
        }
    }
    println!();

    // Test reading CHR data through the mapper
    println!("\n=== Testing CHR reads through mapper ===");
    println!("Reading addresses 0x0000-0x000F:");
    for addr in 0x0000..=0x000F {
        let val = cartridge.read_chr(addr);
        print!("{:02X} ", val);
    }
    println!();

    // Test reading tile 0x24 from pattern table 1
    println!("\nReading tile 0x24 from pattern table 1 (0x1240-0x124F):");
    println!("  Via read_chr:");
    for addr in 0x1240..=0x124F {
        let val = cartridge.read_chr(addr);
        print!("{:02X} ", val);
    }
    println!();
    println!("  Directly from chr_rom:");
    for i in 0x1240..=0x124F {
        if i < cartridge.chr_rom.len() {
            print!("{:02X} ", cartridge.chr_rom[i]);
        } else {
            print!("XX ");
        }
    }
    println!();

    // Check for non-zero bytes in second pattern table
    println!("\nScanning second pattern table (0x1000-0x1FFF) for non-zero bytes:");
    let mut non_zero_count = 0;
    for i in 0x1000..0x2000 {
        if i < cartridge.chr_rom.len() && cartridge.chr_rom[i] != 0 {
            if non_zero_count < 10 {
                println!("  Offset 0x{:04X}: 0x{:02X}", i, cartridge.chr_rom[i]);
            }
            non_zero_count += 1;
        }
    }
    println!("Total non-zero bytes in second pattern table: {}", non_zero_count);

    Ok(())
}
