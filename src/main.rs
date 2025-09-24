mod ppu;
mod apu;
mod cartridge;
mod input;
mod system;

use sdl2::pixels::PixelFormatEnum;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::render::TextureCreator;
use sdl2::video::WindowContext;
use sdl2::audio::{AudioCallback, AudioSpecDesired};
use std::env;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use anyhow::Result;

use crate::cartridge::Cartridge;
use crate::input::ControllerButton;
use crate::system::System;
use crate::ppu::{SCREEN_WIDTH, SCREEN_HEIGHT};

const SCALE: u32 = 3;

struct ApuAudioCallback {
    audio_buffer: Arc<Mutex<VecDeque<f32>>>,
}

impl AudioCallback for ApuAudioCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        let mut buffer = self.audio_buffer.lock().unwrap();
        for sample in out.iter_mut() {
            *sample = buffer.pop_front().unwrap_or(0.0);
        }
    }
}

fn map_keycode_to_button(key: Keycode) -> Option<ControllerButton> {
    match key {
        Keycode::Z => Some(ControllerButton::A),
        Keycode::X => Some(ControllerButton::B),
        Keycode::RShift => Some(ControllerButton::SELECT),
        Keycode::Return => Some(ControllerButton::START),
        Keycode::Up => Some(ControllerButton::UP),
        Keycode::Down => Some(ControllerButton::DOWN),
        Keycode::Left => Some(ControllerButton::LEFT),
        Keycode::Right => Some(ControllerButton::RIGHT),
        _ => None,
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <rom_file>", args[0]);
        std::process::exit(1);
    }

    let rom_path = &args[1];
    log::info!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;
    log::info!("ROM loaded successfully. Mapper: {}", cartridge.mapper);

    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let video_subsystem = sdl_context.video().map_err(|e| anyhow::anyhow!("Video subsystem failed: {}", e))?;
    let audio_subsystem = sdl_context.audio().map_err(|e| anyhow::anyhow!("Audio subsystem failed: {}", e))?;

    let window = video_subsystem
        .window(
            "NES Emulator",
            SCREEN_WIDTH as u32 * SCALE,
            SCREEN_HEIGHT as u32 * SCALE,
        )
        .position_centered()
        .build()
        .map_err(|e| anyhow::anyhow!("Window creation failed: {}", e))?;

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| anyhow::anyhow!("Canvas creation failed: {}", e))?;

    let texture_creator: TextureCreator<WindowContext> = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::RGB24,
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
        )
        .map_err(|e| anyhow::anyhow!("Texture creation failed: {}", e))?;

    let mut event_pump = sdl_context.event_pump().map_err(|e| anyhow::anyhow!("Event pump failed: {}", e))?;

    // Setup audio
    let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(16384)));
    let audio_buffer_clone = Arc::clone(&audio_buffer);
    
    let desired_spec = AudioSpecDesired {
        freq: Some(44100),
        channels: Some(1),
        samples: Some(1024),  // Larger buffer for smoother playback
    };
    
    let audio_device = audio_subsystem
        .open_playback(None, &desired_spec, |_spec| {
            ApuAudioCallback {
                audio_buffer: audio_buffer_clone,
            }
        })
        .map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;
    
    audio_device.resume();

    let mut system = System::new();
    system.load_cartridge(cartridge);

    let frame_duration = Duration::from_nanos(16_666_667);
    let mut _last_frame = Instant::now();

    log::info!("Starting emulation...");

    'running: loop {
        let frame_start = Instant::now();

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(keycode),
                    ..
                } => {
                    if keycode == Keycode::Escape {
                        break 'running;
                    }
                    if keycode == Keycode::R {
                        log::info!("Resetting NES...");
                        system.reset();
                    }
                    if let Some(button) = map_keycode_to_button(keycode) {
                        system.controller1.press(button);
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    ..
                } => {
                    if let Some(button) = map_keycode_to_button(keycode) {
                        system.controller1.release(button);
                    }
                }
                _ => {}
            }
        }

        system.run_frame_with_audio(Some(&audio_buffer));

        texture
            .update(None, system.get_frame_buffer(), SCREEN_WIDTH * 3)
            .map_err(|e| anyhow::anyhow!("Texture update failed: {}", e))?;

        canvas.clear();
        canvas.copy(&texture, None, None)
            .map_err(|e| anyhow::anyhow!("Canvas copy failed: {}", e))?;
        canvas.present();

        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        } else {
            log::debug!("Frame took too long: {:?}", elapsed);
        }
        
        _last_frame = Instant::now();
    }

    log::info!("Emulation stopped.");
    Ok(())
}
