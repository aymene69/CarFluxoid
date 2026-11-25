//! CarFluxoid pro automotive HMI powered by Rust + Slint.
//!
//! Runs either:
//! - headless on Raspberry Pi Zero 2 W (`run_daemon`) using Slint's zero-GPU
//!   software renderer + H.264 SPS/PPS alignment for Android Auto projection,
//! - as an interactive native preview window on macOS/Linux (`--features preview`),
//! - or in screenshot mode (`render_screenshots`) for visual verification.

pub mod h264_align;
pub mod state;

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::unix::net::UnixDatagram,
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use slint::{
    ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer,
    platform::{
        Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};

use crate::{
    h264_align::{align_h264, extract_target_config},
    state::{DashboardController, Paths, atomic_write},
};

slint::include_modules!();

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

fn load_logo_image(settings_dir: &Path) -> Option<slint::Image> {
    let bytes = fs::read(settings_dir.join("logo.png")).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
    if img.width() > 512 || img.height() > 512 {
        return None;
    }
    let rgba = img.to_rgba8();
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(rgba.width(), rgba.height());
    buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
    Some(slint::Image::from_rgba8(buf))
}

fn sync_ui(ui: &DashboardWindow, ctrl: &DashboardController, waiting: bool) {
    let tel = ctrl.telemetry();
    ui.set_waiting_mode(waiting);
    ui.set_active_page(ctrl.active_page.clone().into());
    ui.set_day_mode(ctrl.effective_day_mode(&tel));

    ui.set_oem_name(ctrl.oem_name().into());
    ui.set_version_text(tel.version.into());
    ui.set_build_text(tel.build.into());
    ui.set_cpu_temp(tel.cpu_temp.into());
    ui.set_gps_status(tel.gps_status.into());
    ui.set_night_status(tel.night_status.into());
    ui.set_display_status(tel.display_status.into());
    ui.set_notice(ctrl.notice.clone().into());

    ui.set_auto_start(ctrl.auto_start_enabled());
    ui.set_sidebar(ctrl.option("sidebar", "left").into());
    ui.set_theme(ctrl.option("theme", "auto").into());
    ui.set_fps(ctrl.option("fps", "30").into());
    ui.set_scale_percent(ctrl.option("scale_percent", "85").into());
    ui.set_wifi_name(ctrl.radio_name("wifi").into());
    ui.set_bluetooth_name(ctrl.radio_name("bluetooth").into());

    ui.set_edit_target(ctrl.edit_target.clone().into());
    ui.set_draft(ctrl.draft.clone().into());
    ui.set_shift(ctrl.shift);

    let obd = ctrl.obd_hud();
    ui.set_obd_connected(obd.connected);
    ui.set_obd_status(obd.status.into());
    ui.set_obd_rpm(obd.rpm.into());
    ui.set_obd_speed(obd.speed.into());
    ui.set_obd_boost(obd.boost.into());
    ui.set_obd_coolant(obd.coolant.into());
    ui.set_obd_fuel(obd.fuel.into());
    ui.set_obd_battery(obd.battery.into());
    ui.set_obd_consumption(obd.consumption.into());
    ui.set_obd_dtc_summary(obd.dtc_summary.into());
}

fn wire_callbacks(
    ui: &DashboardWindow,
    ctrl: Rc<RefCell<DashboardController>>,
    dirty: Rc<RefCell<bool>>,
) {
    let ui_weak = ui.as_weak();
    let refresh = move |c: &DashboardController, waiting: bool| {
        if let Some(u) = ui_weak.upgrade() {
            sync_ui(&u, c, waiting);
        }
    };

    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_navigate(move |page| {
            let mut c = ctrl.borrow_mut();
            c.cancel_pending_auto_start();
            c.notice.clear();
            c.active_page = page.to_string();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_start_carplay(move || {
            let mut c = ctrl.borrow_mut();
            c.start_carplay(false);
            let show_wait = c.dry_run;
            refresh(&c, show_wait);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_restart_carplay(move || {
            let mut c = ctrl.borrow_mut();
            c.start_carplay(true);
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_return_radio(move || {
            let mut c = ctrl.borrow_mut();
            c.cancel_pending_auto_start();
            c.shell_action(6);
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_wait_back(move || {
            let mut c = ctrl.borrow_mut();
            c.cancel_pending_auto_start();
            c.active_page = "home".into();
            c.shell_action(8);
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_toggle_auto_start(move || {
            let mut c = ctrl.borrow_mut();
            let next = if c.auto_start_enabled() {
                "false"
            } else {
                "true"
            };
            c.set_option("auto_start", next);
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_set_sidebar(move |val| {
            let mut c = ctrl.borrow_mut();
            c.set_option("sidebar", val.as_str());
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_set_theme(move |val| {
            let mut c = ctrl.borrow_mut();
            c.set_option("theme", val.as_str());
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_set_fps(move |val| {
            let mut c = ctrl.borrow_mut();
            c.set_option("fps", val.as_str());
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_set_scale(move |val| {
            let mut c = ctrl.borrow_mut();
            c.set_option("scale_percent", val.as_str());
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_open_editor(move |kind| {
            let mut c = ctrl.borrow_mut();
            c.open_editor(kind.as_str());
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_press(move |ch| {
            let mut c = ctrl.borrow_mut();
            c.notice.clear();
            if c.draft.chars().count() < 32 {
                c.draft.push_str(ch.as_str());
            }
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_shift(move || {
            let mut c = ctrl.borrow_mut();
            c.shift = !c.shift;
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_erase(move || {
            let mut c = ctrl.borrow_mut();
            c.draft.pop();
            c.notice.clear();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_clear(move || {
            let mut c = ctrl.borrow_mut();
            c.draft.clear();
            c.notice.clear();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_cancel(move || {
            let mut c = ctrl.borrow_mut();
            c.notice.clear();
            c.active_page = "network".into();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_key_save(move || {
            let mut c = ctrl.borrow_mut();
            c.save_editor();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
}

fn render_rgb24(
    window: &MinimalSoftwareWindow,
    width: u32,
    height: u32,
    buf: &mut Vec<Rgb8Pixel>,
) -> Vec<u8> {
    let scale = width as f32 / 800.0;
    window.set_size(PhysicalSize::new(width, height));
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    slint::platform::update_timers_and_animations();
    buf.resize((width * height) as usize, Rgb8Pixel::new(0, 0, 0));
    window.draw_if_needed(|renderer| {
        renderer.render(buf, width as usize);
    });
    // Force full repaint if draw_if_needed skipped (e.g. first call or size unchanged)
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(buf, width as usize);
    });

    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for p in buf.iter() {
        rgb.push(p.r);
        rgb.push(p.g);
        rgb.push(p.b);
    }
    rgb
}

fn encode_h264_aligned(
    rgb24: &[u8],
    width: u32,
    height: u32,
    reference_h264: &Path,
    cache: &mut HashMap<(u32, u32), (Vec<u8>, i32)>,
) -> Result<Vec<u8>, String> {
    let (target_cfg, chroma) = if let Some(entry) = cache.get(&(width, height)) {
        (entry.0.clone(), entry.1)
    } else {
        let ref_bytes = fs::read(reference_h264)
            .map_err(|e| format!("read {}: {e}", reference_h264.display()))?;
        let parsed = extract_target_config(&ref_bytes).map_err(|e| e.to_string())?;
        cache.insert((width, height), parsed.clone());
        parsed
    };

    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "30",
            "-i",
            "pipe:0",
            "-vf",
            "tpad=stop_mode=clone:stop=1",
            "-frames:v",
            "2",
            "-an",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "17",
            "-profile:v",
            "high",
            "-threads",
            "2",
            "-x264-params",
            &format!(
                "keyint=1:bframes=0:cabac=1:8x8dct=1:chroma-qp-offset={chroma}:deblock=0,0"
            ),
            "-f",
            "h264",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;

    let stdin = child.stdin.take();
    let input_bytes = rgb24.to_vec();
    let writer = std::thread::spawn(move || -> Result<(), String> {
        if let Some(mut stdin) = stdin {
            stdin
                .write_all(&input_bytes)
                .map_err(|e| format!("ffmpeg stdin: {e}"))?;
        }
        Ok(())
    });

    let mut raw_h264 = Vec::with_capacity(96 * 1024);
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut raw_h264)
            .map_err(|e| format!("ffmpeg stdout: {e}"))?;
    }

    writer
        .join()
        .map_err(|_| "ffmpeg stdin thread panicked".to_string())??;

    let status = child.wait().map_err(|e| format!("ffmpeg wait: {e}"))?;
    if !status.success() {
        let mut err_msg = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err_msg);
        }
        return Err(format!("ffmpeg exited {status}: {err_msg}"));
    }

    align_h264(&raw_h264, &target_cfg).map_err(|e| format!("h264 align: {e}"))
}

fn init_headless_window() -> Result<Rc<MinimalSoftwareWindow>, String> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let platform = HeadlessPlatform {
        window: window.clone(),
        start: Instant::now(),
    };
    slint::platform::set_platform(Box::new(platform))
        .map_err(|e| format!("set_platform: {e}"))?;
    Ok(window)
}

/// Run headless CarFluxoid projected dashboard daemon on Raspberry Pi.
pub fn run_daemon(paths: Paths) -> Result<(), String> {
    let window = init_headless_window()?;
    fs::create_dir_all(&paths.runtime).map_err(|e| format!("mkdir runtime: {e}"))?;

    let ctrl = Rc::new(RefCell::new(DashboardController::new(paths.clone(), false)));
    let dirty = Rc::new(RefCell::new(true));

    let ui = DashboardWindow::new().map_err(|e| format!("DashboardWindow::new: {e}"))?;
    if let Some(logo) = load_logo_image(&paths.settings) {
        ui.set_car_logo(logo);
        ui.set_has_car_logo(true);
    }
    wire_callbacks(&ui, ctrl.clone(), dirty.clone());
    sync_ui(&ui, &ctrl.borrow(), false);

    let endpoint = paths.runtime.join("input.sock");
    let _ = fs::remove_file(&endpoint);
    let server = UnixDatagram::bind(&endpoint)
        .map_err(|e| format!("bind {}: {e}", endpoint.display()))?;
    server
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| format!("set_read_timeout: {e}"))?;

    let mut pixel_buf = Vec::new();
    let mut cfg_cache: HashMap<(u32, u32), (Vec<u8>, i32)> = HashMap::new();
    let mut last_telemetry_refresh = Instant::now();
    let mut last_logo_mtime = fs::metadata(paths.settings.join("logo.png"))
        .and_then(|m| m.modified())
        .ok();

    let publish = |ui: &DashboardWindow,
                   window: &MinimalSoftwareWindow,
                   ctrl: &mut DashboardController,
                   pixel_buf: &mut Vec<Rgb8Pixel>,
                   cfg_cache: &mut HashMap<(u32, u32), (Vec<u8>, i32)>| {
        let (w, h) = ctrl.dimensions();
        ctrl.size = (w, h);
        let ref_h264 = ctrl.paths.assets.join(format!("attente-{w}x{h}.h264"));

        // 1. Render & publish active menu screen
        sync_ui(ui, ctrl, false);
        let rgb = render_rgb24(window, w, h, pixel_buf);
        match encode_h264_aligned(&rgb, w, h, &ref_h264, cfg_cache) {
            Ok(h264) => {
                let target = ctrl.paths.runtime.join(format!("dashboard-{w}x{h}.h264"));
                if let Err(e) = atomic_write(&target, &h264, false) {
                    eprintln!("DASHBOARD_WRITE_ERR {}: {e}", target.display());
                }
            }
            Err(e) => eprintln!("DASHBOARD_ENCODE_ERR {e}"),
        }

        // 2. Render & publish waiting screen when resolution or BT name changed
        let wait_key = ((w, h), ctrl.radio_name("bluetooth"));
        let wait_path = ctrl.paths.runtime.join(format!("dashboard-wait-{w}x{h}.h264"));
        if ctrl.waiting_key.as_ref() != Some(&wait_key) || !wait_path.is_file() {
            sync_ui(ui, ctrl, true);
            let wait_rgb = render_rgb24(window, w, h, pixel_buf);
            match encode_h264_aligned(&wait_rgb, w, h, &ref_h264, cfg_cache) {
                Ok(h264) => {
                    if let Err(e) = atomic_write(&wait_path, &h264, false) {
                        eprintln!("DASHBOARD_WAIT_WRITE_ERR {}: {e}", wait_path.display());
                    } else {
                        ctrl.waiting_key = Some(wait_key);
                    }
                }
                Err(e) => eprintln!("DASHBOARD_WAIT_ENCODE_ERR {e}"),
            }
            // Restore menu state in Slint tree so touch coordinates always match menu layout
            sync_ui(ui, ctrl, false);
            window.request_redraw();
            let _ = render_rgb24(window, w, h, pixel_buf);
        }
    };

    publish(&ui, &window, &mut ctrl.borrow_mut(), &mut pixel_buf, &mut cfg_cache);
    *dirty.borrow_mut() = false;

    let mut rx = [0u8; 32];
    loop {
        if let Ok(n) = server.recv(&mut rx) {
            if n == 5 {
                let raw_action = rx[0];
                let waiting = (raw_action & 0x80) != 0;
                let action = raw_action & 0x7f;
                let px = u16::from_le_bytes([rx[1], rx[2]]) as f32;
                let py = u16::from_le_bytes([rx[3], rx[4]]) as f32;
                let (w, h) = ctrl.borrow().size;
                let lx = px * 800.0 / (w.max(1) as f32);
                let ly = py * 480.0 / (h.max(1) as f32);
                let pos = LogicalPosition::new(lx, ly);

                if waiting {
                    // Dispatch into waiting layout so "Retour au menu" hit-tests cleanly
                    sync_ui(&ui, &ctrl.borrow(), true);
                    window.request_redraw();
                    window.draw_if_needed(|r| {
                        r.render(&mut pixel_buf, w as usize);
                    });
                }

                let event = match action {
                    0 => Some(WindowEvent::PointerPressed {
                        position: pos,
                        button: PointerEventButton::Left,
                    }),
                    1 => Some(WindowEvent::PointerReleased {
                        position: pos,
                        button: PointerEventButton::Left,
                    }),
                    2 => Some(WindowEvent::PointerMoved { position: pos }),
                    _ => None,
                };
                if let Some(ev) = event {
                    window.dispatch_event(ev);
                }

                if waiting && !*dirty.borrow() {
                    sync_ui(&ui, &ctrl.borrow(), false);
                }
            }
        }

        let mut need_publish = *dirty.borrow();
        {
            let mut c = ctrl.borrow_mut();
            if c.check_carplay_job() {
                need_publish = true;
            }
            if c.maybe_auto_start() {
                need_publish = true;
            }
            if c.size != c.dimensions() {
                need_publish = true;
            }
        }

        // Refresh live telemetry (CPU temp / GPS / Day-Night) every 15 s when on Home/System
        if last_telemetry_refresh.elapsed() >= Duration::from_secs(15) {
            last_telemetry_refresh = Instant::now();
            let cur_logo_mtime = fs::metadata(paths.settings.join("logo.png"))
                .and_then(|m| m.modified())
                .ok();
            if cur_logo_mtime != last_logo_mtime {
                last_logo_mtime = cur_logo_mtime;
                if let Some(logo) = load_logo_image(&paths.settings) {
                    ui.set_car_logo(logo);
                    ui.set_has_car_logo(true);
                }
                need_publish = true;
            } else {
                let page = ctrl.borrow().active_page.clone();
                if page == "home" || page == "system" {
                    need_publish = true;
                }
            }
        }

        if need_publish {
            publish(&ui, &window, &mut ctrl.borrow_mut(), &mut pixel_buf, &mut cfg_cache);
            *dirty.borrow_mut() = false;
        }
    }
}

/// Render all screens to PNG files in `out_dir` for crisp visual verification.
pub fn render_screenshots(out_dir: &Path, logo_path: Option<&Path>) -> Result<(), String> {
    let window = init_headless_window()?;
    fs::create_dir_all(out_dir).map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;

    let mut ctrl = DashboardController::new(Paths::default(), true);
    ctrl.notice.clear();

    let ui = DashboardWindow::new().map_err(|e| format!("DashboardWindow::new: {e}"))?;
    if let Some(lp) = logo_path {
        if let Ok(bytes) = fs::read(lp) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.resize(128, 128, image::imageops::FilterType::Lanczos3).to_rgba8();
                let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(rgba.width(), rgba.height());
                buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
                ui.set_car_logo(slint::Image::from_rgba8(buf));
                ui.set_has_car_logo(true);
            }
        }
    }

    let mut pixel_buf = Vec::new();
    let shots: [(&str, &str, bool, bool); 6] = [
        ("01_home_night.png", "home", false, false),
        ("02_settings_night.png", "settings", false, false),
        ("03_network_night.png", "network", false, false),
        ("04_keyboard_night.png", "keyboard", false, false),
        ("05_system_night.png", "system", false, false),
        ("06_waiting_carplay.png", "home", true, false),
    ];

    for (filename, page, waiting, day) in shots {
        ctrl.active_page = page.to_string();
        if page == "keyboard" {
            ctrl.edit_target = "wifi".into();
            ctrl.draft = "CarFluxoid-Pi_AP".into();
        }
        sync_ui(&ui, &ctrl, waiting);
        ui.set_oem_name("Fiat".into());
        ui.set_day_mode(day);

        let rgb = render_rgb24(&window, 800, 480, &mut pixel_buf);
        let img = image::RgbImage::from_raw(800, 480, rgb)
            .ok_or_else(|| "failed to build RgbImage".to_string())?;
        let dest = out_dir.join(filename);
        img.save(&dest)
            .map_err(|e| format!("save {}: {e}", dest.display()))?;
        println!("Saved {}", dest.display());
    }
    Ok(())
}

/// Interactive 800x480 native desktop preview window (macOS / Linux desktop).
#[cfg(feature = "preview")]
pub fn run_preview(logo_path: Option<&Path>) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("carfluxoid-preview-settings");
    let _ = fs::create_dir_all(&tmp);
    let paths = Paths {
        settings: tmp.clone(),
        assets: Path::new("assets/waiting").to_path_buf(),
        runtime: tmp.clone(),
        bridge: tmp,
    };
    let ctrl = Rc::new(RefCell::new(DashboardController::new(paths, true)));
    let dirty = Rc::new(RefCell::new(false));
    let ui = DashboardWindow::new().map_err(|e| format!("DashboardWindow::new: {e}"))?;

    if let Some(lp) = logo_path {
        if let Ok(bytes) = fs::read(lp) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.resize(128, 128, image::imageops::FilterType::Lanczos3).to_rgba8();
                let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(rgba.width(), rgba.height());
                buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
                ui.set_car_logo(slint::Image::from_rgba8(buf));
                ui.set_has_car_logo(true);
            }
        }
    }

    wire_callbacks(&ui, ctrl.clone(), dirty);
    sync_ui(&ui, &ctrl.borrow(), false);
    ui.set_oem_name("Fiat".into());
    ui.run().map_err(|e| format!("ui.run: {e}"))
}

/// Self-test: verify headless Slint software rendering works cleanly in memory.
pub fn self_test() -> Result<(), String> {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    let _guard = TEST_LOCK.lock().ok();
    let window = init_headless_window()?;
    let ui = DashboardWindow::new().map_err(|e| format!("DashboardWindow::new: {e}"))?;
    let mut buf = Vec::new();
    let rgb = render_rgb24(&window, 800, 480, &mut buf);
    if rgb.len() != 800 * 480 * 3 {
        return Err("unexpected RGB buffer size".into());
    }
    if rgb.iter().all(|&b| b == 0) {
        return Err("rendered blank black frame".into());
    }
    drop(ui);
    Ok(())
}

// Keep Arc imported for future background OBD polling worker integration.
#[allow(dead_code)]
type _SharedReady = Arc<Mutex<()>>;
