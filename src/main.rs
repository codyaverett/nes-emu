mod ui;

use anyhow::{bail, Context, Result};
use sdl2::audio::{AudioCallback, AudioSpecDesired};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, TextureCreator, WindowCanvas};
use sdl2::video::WindowContext;
use std::collections::VecDeque;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nes_emu::cartridge::Cartridge;
use nes_emu::input::ControllerButton;
use nes_emu::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use nes_emu::system::System;

use ui::app::{App, AudioQueue};
use ui::tools::ToolId;
use ui::Ui;

const SCALE: u32 = 3;

/// Samples the emulator keeps queued ahead of the audio device when audio
/// is the master clock: about 3.3 NES frames (roughly 55 ms of latency).
/// The device pulls `AUDIO_DEVICE_SAMPLES` per callback, so the queue can
/// absorb several callbacks between two presents without running dry, and
/// it never approaches the queue's hard cap.
const AUDIO_TARGET_SAMPLES: usize = 2450;
/// Samples per SDL callback. Smaller callbacks mean smaller dips in the
/// queue between presents.
const AUDIO_DEVICE_SAMPLES: u16 = 512;

/// How often battery-backed PRG RAM is flushed to the .sav file while
/// running. `System::save_battery` only writes when the RAM changed.
const BATTERY_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// First presented frame on which `--ui-script` injects a key. Gives the
/// game a moment to draw something behind the overlay.
const UI_SCRIPT_START_FRAME: u64 = 30;

struct ApuAudioCallback {
    audio_buffer: AudioQueue,
    muted: Arc<Mutex<bool>>,
    volume: Arc<Mutex<f32>>,
    /// Last sample delivered; repeated if the queue runs dry so an
    /// underrun holds the waveform instead of snapping to zero.
    last: f32,
}

impl AudioCallback for ApuAudioCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        let mut buffer = self.audio_buffer.lock().unwrap();
        let muted = *self.muted.lock().unwrap();
        let volume = *self.volume.lock().unwrap();

        for sample in out.iter_mut() {
            if let Some(next) = buffer.pop_front() {
                self.last = next;
            }
            *sample = if muted { 0.0 } else { self.last * volume };
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

/// Command-line options.
struct Options {
    rom_path: String,
    enable_audio: bool,
    full_frame: bool,
    /// `(path, frame)`: write the composed window as PPM on that frame.
    screenshots: Vec<(PathBuf, u64)>,
    /// Keys injected one per frame from `UI_SCRIPT_START_FRAME` on;
    /// `None` leaves that frame without input.
    ui_script: Vec<Option<Keycode>>,
}

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage: {} <rom_file> [--no-audio] [--full-frame] [--screenshot PATH:N] [--ui-script KEYS]",
        program
    );
    eprintln!("Battery-backed games read and write <rom_file with .sav extension>.");
    eprintln!("Cheats are read from and written to <rom_file with .cht extension>.");
    eprintln!("Backquote opens the command palette; F1 opens the help page.");
    eprintln!("--screenshot PATH:N writes the window as binary PPM after N presented frames.");
    eprintln!(
        "--ui-script KEYS injects comma-separated SDL key names, one per frame from frame 30."
    );
    std::process::exit(1);
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut rom_path = None;
    let mut enable_audio = true;
    let mut full_frame = false;
    let mut screenshots = Vec::new();
    let mut ui_script = Vec::new();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-audio" => enable_audio = false,
            "--full-frame" => full_frame = true,
            "--screenshot" => {
                let spec = iter.next().context("--screenshot needs PATH:N")?;
                screenshots.push(parse_screenshot_spec(spec)?);
            }
            "--ui-script" => {
                let spec = iter.next().context("--ui-script needs a key list")?;
                ui_script = parse_ui_script(spec)?;
            }
            other if other.starts_with("--") => bail!("Unknown option {}", other),
            other => {
                if rom_path.replace(other.to_string()).is_some() {
                    bail!("More than one ROM path given");
                }
            }
        }
    }
    let rom_path = match rom_path {
        Some(p) => p,
        None => usage(&args[0]),
    };
    Ok(Options {
        rom_path,
        enable_audio,
        full_frame,
        screenshots,
        ui_script,
    })
}

/// `PATH:N`, split at the last colon so paths containing colons survive.
fn parse_screenshot_spec(spec: &str) -> Result<(PathBuf, u64)> {
    let (path, frame) = spec.rsplit_once(':').context("--screenshot wants PATH:N")?;
    let frame: u64 = frame
        .parse()
        .with_context(|| format!("bad frame number in --screenshot {}", spec))?;
    Ok((PathBuf::from(path), frame))
}

/// Comma-separated SDL key names (`Keycode::from_name`), with a few
/// friendlier aliases. An empty entry is a frame with no key.
fn parse_ui_script(spec: &str) -> Result<Vec<Option<Keycode>>> {
    spec.split(',')
        .map(|name| {
            let name = name.trim();
            if name.is_empty() {
                return Ok(None);
            }
            let canonical = match name.to_ascii_lowercase().as_str() {
                "backquote" | "grave" => "`",
                "enter" => "Return",
                "esc" => "Escape",
                _ => name,
            };
            Keycode::from_name(canonical)
                .map(Some)
                .with_context(|| format!("unknown key name {:?} in --ui-script", name))
        })
        .collect()
}

/// Key press routing: the UI first, then hotkeys, then the controller.
fn key_down(key: Keycode, app: &mut App, ui: &mut Ui) {
    if ui.handle_key(key, app) {
        return;
    }
    match key {
        Keycode::Escape => app.quit(),
        Keycode::F1 => ui.open_tool(ToolId::Help),
        Keycode::R => app.reset(),
        Keycode::P => app.toggle_pause(),
        Keycode::N => app.request_frame_advance(),
        Keycode::M => app.toggle_mute(),
        Keycode::Equals | Keycode::Plus => app.volume_up(),
        Keycode::Minus => app.volume_down(),
        _ => {}
    }
    if let Some(button) = map_keycode_to_button(key) {
        app.system.controller1.press(button);
    }
}

/// Releases reach the controller in every UI mode so a button held when
/// the palette opened does not stick.
fn key_up(key: Keycode, app: &mut App) {
    if let Some(button) = map_keycode_to_button(key) {
        app.system.controller1.release(button);
    }
}

/// Write the current render target as a binary PPM. Must run before
/// `present`, after which the back buffer is undefined.
fn write_screenshot(canvas: &WindowCanvas, path: &Path) -> Result<()> {
    let (w, h) = canvas
        .output_size()
        .map_err(|e| anyhow::anyhow!("output size: {}", e))?;
    let pixels = canvas
        .read_pixels(None, PixelFormatEnum::RGB24)
        .map_err(|e| anyhow::anyhow!("read pixels: {}", e))?;
    let mut file =
        std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    write!(file, "P6\n{} {}\n255\n", w, h)?;
    file.write_all(&pixels[..(w * h * 3) as usize])?;
    log::info!("Wrote screenshot {} ({}x{})", path.display(), w, h);
    Ok(())
}

fn draw_osd(canvas: &mut WindowCanvas, app: &App) -> Result<()> {
    let osd_x = 10 * SCALE as i32;
    let osd_y = 10 * SCALE as i32;
    let osd_width = 200 * SCALE;
    let osd_height = 20 * SCALE;

    // Background (semi-transparent black)
    canvas.set_draw_color(Color::RGBA(0, 0, 0, 180));
    canvas
        .fill_rect(Rect::new(osd_x, osd_y, osd_width, osd_height))
        .map_err(|e| anyhow::anyhow!("Failed to draw OSD background: {}", e))?;

    if app.is_muted() {
        // Muted indicator (red)
        canvas.set_draw_color(Color::RGB(255, 0, 0));
        canvas
            .fill_rect(Rect::new(
                osd_x + 2 * SCALE as i32,
                osd_y + 2 * SCALE as i32,
                osd_width - 4 * SCALE,
                osd_height - 4 * SCALE,
            ))
            .map_err(|e| anyhow::anyhow!("Failed to draw mute indicator: {}", e))?;
    } else {
        // Volume bar (green)
        let filled_width = ((osd_width - 4 * SCALE) as f32 * app.volume()) as u32;
        canvas.set_draw_color(Color::RGB(0, 255, 0));
        canvas
            .fill_rect(Rect::new(
                osd_x + 2 * SCALE as i32,
                osd_y + 2 * SCALE as i32,
                filled_width,
                osd_height - 4 * SCALE,
            ))
            .map_err(|e| anyhow::anyhow!("Failed to draw volume bar: {}", e))?;
    }
    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
    }
    let options = parse_options(&args)?;
    let rom_path = &options.rom_path;

    if !options.enable_audio {
        log::info!("Audio disabled via command-line flag");
    }

    log::info!("Loading ROM: {}", rom_path);

    let cartridge = Cartridge::load_from_file(rom_path)?;
    log::info!("ROM loaded successfully. Mapper: {}", cartridge.mapper_id);

    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let video_subsystem = sdl_context
        .video()
        .map_err(|e| anyhow::anyhow!("Video subsystem failed: {}", e))?;

    // Setup audio (conditional)
    let muted = Arc::new(Mutex::new(false));
    let volume = Arc::new(Mutex::new(0.5f32)); // Start at 50% volume

    let (audio_buffer, _audio_device) = if options.enable_audio {
        let audio_subsystem = sdl_context
            .audio()
            .map_err(|e| anyhow::anyhow!("Audio subsystem failed: {}", e))?;
        let buffer: AudioQueue = Arc::new(Mutex::new(VecDeque::with_capacity(16384)));
        let buffer_clone = Arc::clone(&buffer);
        let muted_clone = Arc::clone(&muted);
        let volume_clone = Arc::clone(&volume);

        let desired_spec = AudioSpecDesired {
            freq: Some(44100),
            channels: Some(1),
            samples: Some(AUDIO_DEVICE_SAMPLES),
        };

        let audio_device = audio_subsystem
            .open_playback(None, &desired_spec, |_spec| ApuAudioCallback {
                last: 0.0,
                audio_buffer: buffer_clone,
                muted: muted_clone,
                volume: volume_clone,
            })
            .map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;

        audio_device.resume();
        (Some(buffer), Some(audio_device))
    } else {
        (None, None)
    };

    let mut system = System::new();
    let battery_backed = cartridge.battery_backed;
    system.load_cartridge(cartridge);

    // Battery-backed PRG RAM lives next to the ROM as <rom>.sav. Loaded
    // once here, flushed every few seconds below and once more on exit.
    let save_path = Path::new(rom_path).with_extension("sav");
    if battery_backed {
        match system.load_battery(&save_path) {
            Ok(true) => {}
            Ok(false) => log::info!("No battery save at {}", save_path.display()),
            Err(e) => log::warn!("Could not read {}: {}", save_path.display(), e),
        }
    }

    // Cheats live next to the ROM as <rom>.cht (docs/debugging/CHEAT_ENGINE.md).
    // Loaded once here; the App rewrites the file on every change.
    let cheat_path = Path::new(rom_path).with_extension("cht");
    match system.load_cheats(&cheat_path) {
        Ok(true) => {}
        Ok(false) => log::info!("No cheat file at {}", cheat_path.display()),
        Err(e) => log::warn!("Could not read {}: {}", cheat_path.display(), e),
    }

    let mut app = App::new(
        system,
        audio_buffer,
        muted,
        volume,
        !options.full_frame,
        save_path,
        cheat_path,
    );
    let mut ui = Ui::new(ui::DEFAULT_FONT_SCALE);

    let (visible_width, visible_height) = app.visible_size();
    let mut src_rect = app.src_rect();

    let window = video_subsystem
        .window(
            "NES Emulator",
            visible_width * SCALE,
            visible_height * SCALE,
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
    // Overlays are translucent; without this every RGBA fill is opaque.
    canvas.set_blend_mode(BlendMode::Blend);

    let texture_creator: TextureCreator<WindowContext> = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::RGB24,
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
        )
        .map_err(|e| anyhow::anyhow!("Texture creation failed: {}", e))?;

    let mut event_pump = sdl_context
        .event_pump()
        .map_err(|e| anyhow::anyhow!("Event pump failed: {}", e))?;

    let mut last_battery_flush = Instant::now();
    let frame_duration = Duration::from_nanos(16_666_667);
    let mut presented: u64 = 0;
    let mut script: VecDeque<Option<Keycode>> = options.ui_script.iter().copied().collect();
    let mut screenshots = options.screenshots.clone();

    log::info!("Starting emulation...");

    'running: loop {
        let frame_start = Instant::now();

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(keycode),
                    ..
                } => key_down(keycode, &mut app, &mut ui),
                Event::KeyUp {
                    keycode: Some(keycode),
                    ..
                } => key_up(keycode, &mut app),
                _ => {}
            }
        }

        // Scripted input: one key per frame, pressed and released.
        if presented >= UI_SCRIPT_START_FRAME {
            if let Some(Some(key)) = script.pop_front() {
                log::info!("ui-script: {}", key);
                key_down(key, &mut app, &mut ui);
                key_up(key, &mut app);
            }
        }

        if app.quit_requested {
            break 'running;
        }

        if app.crop_dirty {
            app.crop_dirty = false;
            src_rect = app.src_rect();
            let (w, h) = app.visible_size();
            if let Err(e) = canvas.window_mut().set_size(w * SCALE, h * SCALE) {
                log::warn!("Could not resize window: {}", e);
            }
        }

        ui.tick(&mut app);

        // With audio enabled the audio device is the master clock: emulate
        // only until enough samples are queued, so production matches the
        // 44.1 kHz consumption exactly and never drifts with sleep jitter
        // or the display's refresh rate. Without audio, one frame per
        // iteration paced by the sleep below. Paused, nothing runs except
        // a single requested frame advance.
        if app.paused {
            if app.frame_advance {
                app.frame_advance = false;
                let buffer = app.audio_buffer.clone();
                app.system.run_frame_with_audio(buffer.as_ref());
            }
        } else {
            match app.audio_buffer.clone() {
                Some(buffer) => {
                    let mut frames = 0;
                    while buffer.lock().unwrap().len() < AUDIO_TARGET_SAMPLES && frames < 4 {
                        app.system.run_frame_with_audio(Some(&buffer));
                        frames += 1;
                    }
                }
                None => {
                    app.system.run_frame();
                }
            }
        }

        texture
            .update(None, app.system.get_frame_buffer(), SCREEN_WIDTH * 3)
            .map_err(|e| anyhow::anyhow!("Texture update failed: {}", e))?;

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        canvas
            .copy(&texture, Some(src_rect), None)
            .map_err(|e| anyhow::anyhow!("Canvas copy failed: {}", e))?;

        // Volume / mute indicator, only while audio is on.
        if let Some(until) = app.osd_until {
            if Instant::now() < until {
                if app.audio_enabled() {
                    draw_osd(&mut canvas, &app)?;
                }
            } else {
                app.osd_until = None;
            }
        }

        ui.draw(&mut canvas, &app)
            .map_err(|e| anyhow::anyhow!("UI draw failed: {}", e))?;

        // Screenshots read the back buffer, so they must precede present.
        let mut i = 0;
        while i < screenshots.len() {
            if screenshots[i].1 == presented {
                let (path, _) = screenshots.remove(i);
                if let Err(e) = write_screenshot(&canvas, &path) {
                    log::warn!("Screenshot failed: {}", e);
                }
            } else {
                i += 1;
            }
        }

        canvas.present();
        presented += 1;

        if last_battery_flush.elapsed() >= BATTERY_FLUSH_INTERVAL {
            last_battery_flush = Instant::now();
            if let Err(e) = app.system.save_battery(&app.save_path) {
                log::warn!("Could not write {}: {}", app.save_path.display(), e);
            }
        }

        let elapsed = frame_start.elapsed();
        if app.audio_enabled() && !app.paused {
            // Audio paces emulation; just avoid spinning between presents.
            if elapsed < Duration::from_millis(2) {
                std::thread::sleep(Duration::from_millis(1));
            }
        } else if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        } else {
            log::debug!("Frame took too long: {:?}", elapsed);
        }
    }

    if let Err(e) = app.system.save_battery(&app.save_path) {
        log::warn!("Could not write {}: {}", app.save_path.display(), e);
    }

    log::info!("Emulation stopped.");
    Ok(())
}
