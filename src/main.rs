mod ppu;
mod apu;
mod cartridge;
mod input;
mod system;

use sdl2::pixels::{PixelFormatEnum, Color};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::render::TextureCreator;
use sdl2::video::WindowContext;
use sdl2::audio::{AudioCallback, AudioSpecDesired, AudioDevice};
use sdl2::rect::Rect;
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
    muted: Arc<Mutex<bool>>,
    volume: Arc<Mutex<f32>>,
}

impl AudioCallback for ApuAudioCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        let mut buffer = self.audio_buffer.lock().unwrap();
        let muted = *self.muted.lock().unwrap();
        let volume = *self.volume.lock().unwrap();

        for sample in out.iter_mut() {
            let raw_sample = buffer.pop_front().unwrap_or(0.0);
            *sample = if muted { 0.0 } else { raw_sample * volume };
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
        eprintln!("Usage: {} <rom_file> [--no-audio]", args[0]);
        std::process::exit(1);
    }

    let rom_path = &args[1];
    let enable_audio = !args.contains(&"--no-audio".to_string());

    if !enable_audio {
        log::info!("Audio disabled via command-line flag");
    }

    log::info!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;
    log::info!("ROM loaded successfully. Mapper: {}", cartridge.mapper);

    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let video_subsystem = sdl_context.video().map_err(|e| anyhow::anyhow!("Video subsystem failed: {}", e))?;

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

    // Setup audio (conditional)
    let muted = Arc::new(Mutex::new(false));
    let volume = Arc::new(Mutex::new(0.5f32)); // Start at 50% volume

    let (audio_buffer, _audio_device) = if enable_audio {
        let audio_subsystem = sdl_context.audio().map_err(|e| anyhow::anyhow!("Audio subsystem failed: {}", e))?;
        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(16384)));
        let buffer_clone = Arc::clone(&buffer);
        let muted_clone = Arc::clone(&muted);
        let volume_clone = Arc::clone(&volume);

        let desired_spec = AudioSpecDesired {
            freq: Some(44100),
            channels: Some(1),
            samples: Some(1024),  // Larger buffer for smoother playback
        };

        let audio_device = audio_subsystem
            .open_playback(None, &desired_spec, |_spec| {
                ApuAudioCallback {
                    audio_buffer: buffer_clone,
                    muted: muted_clone,
                    volume: volume_clone,
                }
            })
            .map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;

        audio_device.resume();
        (Some(buffer), Some(audio_device))
    } else {
        (None, None)
    };

    let mut system = System::new();
    system.load_cartridge(cartridge);

    let frame_duration = Duration::from_nanos(16_666_667);
    let mut _last_frame = Instant::now();
    let mut osd_shown_until: Option<Instant> = None;

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
                    // Audio controls
                    if enable_audio {
                        if keycode == Keycode::M {
                            let mut m = muted.lock().unwrap();
                            *m = !*m;
                            log::info!("Audio {}", if *m { "muted" } else { "unmuted" });
                            osd_shown_until = Some(Instant::now() + Duration::from_secs(2));
                        }
                        if keycode == Keycode::Equals || keycode == Keycode::Plus {
                            let mut v = volume.lock().unwrap();
                            *v = (*v + 0.1).min(1.0);
                            log::info!("Volume: {:.0}%", *v * 100.0);
                            osd_shown_until = Some(Instant::now() + Duration::from_secs(2));
                        }
                        if keycode == Keycode::Minus {
                            let mut v = volume.lock().unwrap();
                            *v = (*v - 0.1).max(0.0);
                            log::info!("Volume: {:.0}%", *v * 100.0);
                            osd_shown_until = Some(Instant::now() + Duration::from_secs(2));
                        }
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

        system.run_frame_with_audio(audio_buffer.as_ref());

        texture
            .update(None, system.get_frame_buffer(), SCREEN_WIDTH * 3)
            .map_err(|e| anyhow::anyhow!("Texture update failed: {}", e))?;

        canvas.clear();
        canvas.copy(&texture, None, None)
            .map_err(|e| anyhow::anyhow!("Canvas copy failed: {}", e))?;

        // Draw OSD if active
        if let Some(until) = osd_shown_until {
            if Instant::now() < until {
                let is_muted = *muted.lock().unwrap();
                let vol = *volume.lock().unwrap();

                // OSD position and size (scaled)
                let osd_x = 10 * SCALE as i32;
                let osd_y = 10 * SCALE as i32;
                let osd_width = 200 * SCALE as u32;
                let osd_height = 20 * SCALE as u32;

                // Background (semi-transparent black)
                canvas.set_draw_color(Color::RGBA(0, 0, 0, 180));
                canvas.fill_rect(Rect::new(osd_x, osd_y, osd_width, osd_height))
                    .map_err(|e| anyhow::anyhow!("Failed to draw OSD background: {}", e))?;

                if is_muted {
                    // Muted indicator (red)
                    canvas.set_draw_color(Color::RGB(255, 0, 0));
                    canvas.fill_rect(Rect::new(osd_x + 2 * SCALE as i32, osd_y + 2 * SCALE as i32,
                                               osd_width - 4 * SCALE, osd_height - 4 * SCALE))
                        .map_err(|e| anyhow::anyhow!("Failed to draw mute indicator: {}", e))?;
                } else {
                    // Volume bar (green)
                    let filled_width = ((osd_width - 4 * SCALE) as f32 * vol) as u32;
                    canvas.set_draw_color(Color::RGB(0, 255, 0));
                    canvas.fill_rect(Rect::new(osd_x + 2 * SCALE as i32, osd_y + 2 * SCALE as i32,
                                               filled_width, osd_height - 4 * SCALE))
                        .map_err(|e| anyhow::anyhow!("Failed to draw volume bar: {}", e))?;
                }
            } else {
                osd_shown_until = None;
            }
        }

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
