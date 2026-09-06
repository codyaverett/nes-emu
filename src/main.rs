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
use ui::key::Key;
use ui::painter::Painter;
use ui::tools::ToolId;
use ui::Ui;

const SCALE: u32 = 3;

/// The window as a `Painter`: every overlay rectangle becomes one
/// `set_draw_color` plus `fill_rect` on the canvas, exactly the calls the
/// UI made before the trait existed, so screenshots are unchanged.
struct SdlPainter<'a> {
    canvas: &'a mut WindowCanvas,
    size: (u32, u32),
}

impl<'a> SdlPainter<'a> {
    fn new(canvas: &'a mut WindowCanvas) -> Result<Self, String> {
        let size = canvas.output_size()?;
        Ok(SdlPainter { canvas, size })
    }
}

impl Painter for SdlPainter<'_> {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn fill_rect(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        colour: ui::painter::Color,
    ) -> Result<(), String> {
        self.canvas
            .set_draw_color(Color::RGBA(colour.r, colour.g, colour.b, colour.a));
        self.canvas.fill_rect(Rect::new(x, y, w, h))
    }
}

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

/// Which controller port a key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Player {
    One,
    Two,
}

/// Default key map for both controllers (README "Controls"). Player 2 uses
/// I/J/K/L for the D-pad, apostrophe/semicolon for A/B and period/comma for
/// Start/Select; none of these are hotkeys in `key_down`, and page-local
/// keys and palette typing only run while the UI owns the keyboard.
fn map_keycode_to_button(key: Keycode) -> Option<(Player, ControllerButton)> {
    use ControllerButton as B;
    use Player::{One, Two};
    match key {
        Keycode::Z => Some((One, B::A)),
        Keycode::X => Some((One, B::B)),
        Keycode::RShift => Some((One, B::SELECT)),
        Keycode::Return => Some((One, B::START)),
        Keycode::Up => Some((One, B::UP)),
        Keycode::Down => Some((One, B::DOWN)),
        Keycode::Left => Some((One, B::LEFT)),
        Keycode::Right => Some((One, B::RIGHT)),
        Keycode::Quote => Some((Two, B::A)),
        Keycode::Semicolon => Some((Two, B::B)),
        Keycode::Comma => Some((Two, B::SELECT)),
        Keycode::Period => Some((Two, B::START)),
        Keycode::I => Some((Two, B::UP)),
        Keycode::K => Some((Two, B::DOWN)),
        Keycode::J => Some((Two, B::LEFT)),
        Keycode::L => Some((Two, B::RIGHT)),
        _ => None,
    }
}

/// The controller a mapped key drives.
fn controller_for(app: &mut App, player: Player) -> &mut nes_emu::input::Controller {
    match player {
        Player::One => &mut app.system.controller1,
        Player::Two => &mut app.system.controller2,
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
    ui_script: Vec<Option<ScriptKey>>,
}

/// One frame of `--ui-script` input: which key, and whether this frame
/// presses and/or releases it. A tapped key does both; `Key*N` presses
/// on its first frame, holds, and releases on frame N.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScriptKey {
    key: Keycode,
    down: bool,
    up: bool,
}

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage: {} <rom_file> [--no-audio] [--full-frame] [--screenshot PATH:N] [--ui-script KEYS]",
        program
    );
    eprintln!("Battery-backed games read and write <rom_file with .sav extension>.");
    eprintln!("Cheats are read from and written to <rom_file with .cht extension>.");
    eprintln!("Without one, the bundled cheats/ directory is searched by ROM CRC-32");
    eprintln!("(override with --cheats-dir PATH); the match is copied next to the ROM.");
    eprintln!("Backquote opens the command palette; F1 opens the help page.");
    eprintln!("Save states: F5 saves, F8 loads, F6/F7 change the slot (<rom_file>.s1 .. .s9).");
    eprintln!("--screenshot PATH:N writes the window as binary PPM after N presented frames.");
    eprintln!(
        "--ui-script KEYS injects comma-separated SDL key names, one per frame from frame 30."
    );
    eprintln!("  KEY*N holds KEY for N frames (Backspace*30 rewinds for half a second).");
    eprintln!("Hold Backspace to rewind; the rewind palette command jumps back N seconds.");
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
/// friendlier aliases. An empty entry is a frame with no key. `KEY*N`
/// expands to N frames: pressed on the first, held, released on the
/// last, so hold-to-act keys (Backspace rewinds) can be scripted.
fn parse_ui_script(spec: &str) -> Result<Vec<Option<ScriptKey>>> {
    let mut out = Vec::new();
    for entry in spec.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            out.push(None);
            continue;
        }
        let (name, frames) = match entry.rsplit_once('*') {
            Some((name, count)) => {
                let frames: usize = count
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n| *n >= 1)
                    .with_context(|| format!("bad hold count in --ui-script {:?}", entry))?;
                (name.trim(), frames)
            }
            None => (entry, 1),
        };
        let canonical = match name.to_ascii_lowercase().as_str() {
            "backquote" | "grave" => "`",
            "enter" => "Return",
            "esc" => "Escape",
            _ => name,
        };
        let key = Keycode::from_name(canonical)
            .with_context(|| format!("unknown key name {:?} in --ui-script", name))?;
        for i in 0..frames {
            out.push(Some(ScriptKey {
                key,
                down: i == 0,
                up: i + 1 == frames,
            }));
        }
    }
    Ok(out)
}

/// The frontend-neutral key for an SDL key code: printable ASCII codes
/// (32..=126, which SDL numbers by their character) become `Key::Char`,
/// named keys map one to one, anything else is `Key::Other`.
fn sdl_key(key: Keycode) -> Key {
    let code = key as i32;
    if key == Keycode::Backquote {
        return Key::Backquote;
    }
    if (32..=126).contains(&code) {
        return char::from_u32(code as u32).map_or(Key::Other, Key::Char);
    }
    match key {
        Keycode::Escape => Key::Escape,
        Keycode::Return | Keycode::KpEnter => Key::Return,
        Keycode::Backspace => Key::Backspace,
        Keycode::Delete => Key::Delete,
        Keycode::Insert => Key::Insert,
        Keycode::Tab => Key::Tab,
        Keycode::Up => Key::Up,
        Keycode::Down => Key::Down,
        Keycode::Left => Key::Left,
        Keycode::Right => Key::Right,
        Keycode::PageUp => Key::PageUp,
        Keycode::PageDown => Key::PageDown,
        Keycode::Home => Key::Home,
        Keycode::End => Key::End,
        Keycode::F1 => Key::F1,
        Keycode::F2 => Key::F2,
        Keycode::F3 => Key::F3,
        Keycode::F4 => Key::F4,
        Keycode::F5 => Key::F5,
        Keycode::F6 => Key::F6,
        Keycode::F7 => Key::F7,
        Keycode::F8 => Key::F8,
        _ => Key::Other,
    }
}

/// Key press routing: the UI first, then hotkeys, then the controller.
/// While the palette or a page is open the UI consumes every press, so
/// nothing reaches the hotkeys or the controller.
fn key_down(keycode: Keycode, app: &mut App, ui: &mut Ui) {
    let key = sdl_key(keycode);
    if ui.handle_key(key, app) {
        return;
    }
    match key {
        Key::Escape => app.quit(),
        Key::F1 => ui.open_tool(ToolId::Help),
        Key::Char('r') => app.reset(),
        Key::Char('p') => app.toggle_pause(),
        Key::Char('n') => app.request_frame_advance(),
        Key::Char('m') => app.toggle_mute(),
        Key::Char('=') | Key::Char('+') => app.volume_up(),
        Key::Char('-') => app.volume_down(),
        // Save states (docs/debugging/SAVE_STATES.md).
        Key::F5 => app.save_state(),
        Key::F6 => app.prev_slot(),
        Key::F7 => app.next_slot(),
        Key::F8 => app.load_state(),
        // Rewind while held (docs/debugging/UI_FRAMEWORK.md, issue #44);
        // the release below ends it.
        Key::Backspace => app.rewind_start(),
        _ => {}
    }
    if let Some((player, button)) = map_keycode_to_button(keycode) {
        controller_for(app, player).press(button);
    }
}

/// One line of text over the bottom of the picture ("Saved slot 3").
fn draw_message(painter: &mut dyn Painter, font_scale: u32, text: &str) -> Result<()> {
    use ui::painter::Color;
    let (w, h) = painter.size();
    let pad = 4 * font_scale as i32;
    let line = ui::font::line_height(font_scale) as i32;
    let y = h as i32 - line - 3 * pad;
    painter
        .fill_rect(
            0,
            y - pad,
            w,
            (line + 2 * pad) as u32,
            Color::rgba(0, 0, 0, 180),
        )
        .map_err(|e| anyhow::anyhow!("Failed to draw message background: {}", e))?;
    ui::font::draw_text(painter, pad, y, font_scale, Color::rgb(255, 255, 255), text)
        .map_err(|e| anyhow::anyhow!("Failed to draw message: {}", e))
}

/// Releases reach the controller in every UI mode so a button held when
/// the palette opened does not stick.
fn key_up(key: Keycode, app: &mut App) {
    if sdl_key(key) == Key::Backspace {
        // A no-op when the press went to the palette or a page.
        app.rewind_stop();
    }
    if let Some((player, button)) = map_keycode_to_button(key) {
        controller_for(app, player).release(button);
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

fn draw_osd(painter: &mut dyn Painter, app: &App) -> Result<()> {
    use ui::painter::Color;
    let osd_x = 10 * SCALE as i32;
    let osd_y = 10 * SCALE as i32;
    let osd_width = 200 * SCALE;
    let osd_height = 20 * SCALE;

    // Background (semi-transparent black)
    painter
        .fill_rect(
            osd_x,
            osd_y,
            osd_width,
            osd_height,
            Color::rgba(0, 0, 0, 180),
        )
        .map_err(|e| anyhow::anyhow!("Failed to draw OSD background: {}", e))?;

    if app.is_muted() {
        // Muted indicator (red)
        painter
            .fill_rect(
                osd_x + 2 * SCALE as i32,
                osd_y + 2 * SCALE as i32,
                osd_width - 4 * SCALE,
                osd_height - 4 * SCALE,
                Color::rgb(255, 0, 0),
            )
            .map_err(|e| anyhow::anyhow!("Failed to draw mute indicator: {}", e))?;
    } else {
        // Volume bar (green)
        let filled_width = ((osd_width - 4 * SCALE) as f32 * app.volume()) as u32;
        painter
            .fill_rect(
                osd_x + 2 * SCALE as i32,
                osd_y + 2 * SCALE as i32,
                filled_width,
                osd_height - 4 * SCALE,
                Color::rgb(0, 255, 0),
            )
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
    let cheats_dir = args
        .iter()
        .position(|a| a == "--cheats-dir")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("cheats"));
    match system.load_cheats(&cheat_path) {
        Ok(true) => log::info!("Loaded cheats from {}", cheat_path.display()),
        Ok(false) => {
            // No working file yet: seed it from the bundled database by CRC.
            let crc = system.cartridge.as_ref().map(|c| c.rom_crc32).unwrap_or(0);
            match nes_emu::cheat::find_in_database(&cheats_dir, crc) {
                Some(db_path) => match system.load_cheats(&db_path) {
                    Ok(true) => {
                        log::info!(
                            "Seeded {} cheats from {} (ROM CRC-32 {:08X}); saved to {}",
                            system.cheats().len(),
                            db_path.display(),
                            crc,
                            cheat_path.display()
                        );
                        if let Err(e) = system.save_cheats(&cheat_path) {
                            log::warn!("Could not write {}: {}", cheat_path.display(), e);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => log::warn!("Could not read {}: {}", db_path.display(), e),
                },
                None => log::info!(
                    "No cheats for ROM CRC-32 {:08X} in {} and no {}",
                    crc,
                    cheats_dir.display(),
                    cheat_path.display()
                ),
            }
        }
        Err(e) => log::warn!("Could not read {}: {}", cheat_path.display(), e),
    }

    let mut app = App::new(
        system,
        audio_buffer,
        muted,
        volume,
        !options.full_frame,
        PathBuf::from(rom_path),
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
    let mut script: VecDeque<Option<ScriptKey>> = options.ui_script.iter().copied().collect();
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

        // Scripted input: one key per frame, pressed and released, or
        // held across frames for a `KEY*N` entry.
        if presented >= UI_SCRIPT_START_FRAME {
            if let Some(Some(entry)) = script.pop_front() {
                if entry.down {
                    log::info!("ui-script: {}", entry.key);
                    key_down(entry.key, &mut app, &mut ui);
                }
                if entry.up {
                    key_up(entry.key, &mut app);
                }
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
        // a single requested frame advance. Rewinding (Backspace held),
        // nothing is emulated: one snapshot is loaded per presented frame
        // and the audio queue is dropped so the device holds its last
        // sample (silence) until emulation refills it on release.
        if app.rewinding {
            if let Some(buffer) = &app.audio_buffer {
                buffer.lock().unwrap().clear();
            }
            app.rewind_step();
            app.osd_text = Some((
                format!("Rewinding  {:.1} s", app.rewind_seconds()),
                Instant::now() + Duration::from_millis(500),
            ));
        } else if app.paused {
            if app.frame_advance {
                app.frame_advance = false;
                let buffer = app.audio_buffer.clone();
                app.system.run_frame_with_audio(buffer.as_ref());
                app.record_rewind_frame();
            }
        } else {
            match app.audio_buffer.clone() {
                Some(buffer) => {
                    let mut frames = 0;
                    while buffer.lock().unwrap().len() < AUDIO_TARGET_SAMPLES && frames < 4 {
                        app.system.run_frame_with_audio(Some(&buffer));
                        app.record_rewind_frame();
                        frames += 1;
                    }
                }
                None => {
                    app.system.run_frame();
                    app.record_rewind_frame();
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

        {
            let mut painter =
                SdlPainter::new(&mut canvas).map_err(|e| anyhow::anyhow!("output size: {}", e))?;

            // Volume / mute indicator, only while audio is on.
            if let Some(until) = app.osd_until {
                if Instant::now() < until {
                    if app.audio_enabled() {
                        draw_osd(&mut painter, &app)?;
                    }
                } else {
                    app.osd_until = None;
                }
            }

            // Toasts for every operation, in every mode; while paused with
            // no toast showing, a persistent reminder of how to resume.
            if let Some(text) = app.osd_message().map(str::to_owned) {
                draw_message(&mut painter, ui.font_scale, &text)?;
            } else {
                app.osd_text = None;
                if app.paused && !app.rewinding {
                    draw_message(
                        &mut painter,
                        ui.font_scale,
                        "PAUSED   P resume   N step   Bksp rewind",
                    )?;
                }
            }

            ui.draw(&mut painter, &app)
                .map_err(|e| anyhow::anyhow!("UI draw failed: {}", e))?;
        }

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
        if app.audio_enabled() && !app.paused && !app.rewinding {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_script_taps_and_holds() {
        let script = parse_ui_script("a,,Backspace*3,esc").unwrap();
        let tap = |key| {
            Some(ScriptKey {
                key,
                down: true,
                up: true,
            })
        };
        assert_eq!(script.len(), 6);
        assert_eq!(script[0], tap(Keycode::A));
        assert_eq!(script[1], None);
        assert_eq!(
            script[2],
            Some(ScriptKey {
                key: Keycode::Backspace,
                down: true,
                up: false
            })
        );
        assert_eq!(
            script[3],
            Some(ScriptKey {
                key: Keycode::Backspace,
                down: false,
                up: false
            })
        );
        assert_eq!(
            script[4],
            Some(ScriptKey {
                key: Keycode::Backspace,
                down: false,
                up: true
            })
        );
        assert_eq!(script[5], tap(Keycode::Escape));
        // A one-frame hold is a tap.
        assert_eq!(parse_ui_script("x*1").unwrap()[0], tap(Keycode::X));
        assert!(parse_ui_script("x*0").is_err());
        assert!(parse_ui_script("x*many").is_err());
        assert!(parse_ui_script("nosuchkey").is_err());
    }

    #[test]
    fn sdl_keys_map_to_ui_keys() {
        assert_eq!(sdl_key(Keycode::A), Key::Char('a'));
        assert_eq!(sdl_key(Keycode::Num3), Key::Char('3'));
        assert_eq!(sdl_key(Keycode::Space), Key::Char(' '));
        assert_eq!(sdl_key(Keycode::Semicolon), Key::Char(';'));
        assert_eq!(sdl_key(Keycode::Equals), Key::Char('='));
        assert_eq!(sdl_key(Keycode::Minus), Key::Char('-'));
        assert_eq!(sdl_key(Keycode::Backquote), Key::Backquote);
        assert_eq!(sdl_key(Keycode::KpEnter), Key::Return);
        assert_eq!(sdl_key(Keycode::Return), Key::Return);
        assert_eq!(sdl_key(Keycode::Tab), Key::Tab);
        assert_eq!(sdl_key(Keycode::PageDown), Key::PageDown);
        assert_eq!(sdl_key(Keycode::F8), Key::F8);
        assert_eq!(sdl_key(Keycode::F9), Key::Other);
        assert_eq!(sdl_key(Keycode::LShift), Key::Other);
        // The same keys through the browser mapping agree.
        for (sdl, code) in [
            (Keycode::A, "KeyA"),
            (Keycode::Num3, "Digit3"),
            (Keycode::Equals, "Equal"),
            (Keycode::Backquote, "Backquote"),
            (Keycode::Return, "Enter"),
            (Keycode::F5, "F5"),
        ] {
            assert_eq!(sdl_key(sdl), Key::from_browser_code(code), "{code}");
        }
    }
}
