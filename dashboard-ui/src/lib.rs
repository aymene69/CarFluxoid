//! CarFluxoid pro automotive HMI powered by Rust + Slint.
//!
//! Runs either:
//! - headless on Raspberry Pi Zero 2 W (`run_daemon`) using Slint's zero-GPU
//!   software renderer + H.264 SPS/PPS alignment for Android Auto projection,
//! - as an interactive native preview window on macOS/Linux (`--features preview`),
//! - or in screenshot mode (`render_screenshots`) for visual verification.

pub mod h264_align;
pub mod qr;
pub mod state;

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::unix::net::UnixDatagram,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
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
    let (inset_x, inset_y) = ctrl.safe_insets();
    ui.set_waiting_mode(waiting);
    ui.set_active_page(ctrl.active_page.clone().into());
    ui.set_day_mode(ctrl.effective_day_mode(&tel));
    ui.set_safe_inset_x(inset_x);
    ui.set_safe_inset_y(inset_y);

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
    ui.set_obd_dpf_regen(obd.dpf_regen);
    ui.set_obd_status(obd.status.into());
    ui.set_obd_rpm(obd.rpm.into());
    ui.set_obd_rpm_ratio(obd.rpm_ratio);
    ui.set_obd_speed(obd.speed.into());
    ui.set_obd_gear(obd.gear.into());
    ui.set_obd_shift_hint(obd.shift_hint.into());
    ui.set_obd_power_torque(obd.power_torque.into());
    ui.set_obd_boost(obd.boost.into());
    ui.set_obd_boost_ratio(obd.boost_ratio);
    ui.set_obd_coolant(obd.coolant.into());
    ui.set_obd_fuel(obd.fuel.into());
    ui.set_obd_fuel_ratio(obd.fuel_ratio);
    ui.set_obd_battery(obd.battery.into());
    ui.set_obd_consumption(obd.consumption.into());
    ui.set_obd_rail_egr(obd.rail_egr.into());
    ui.set_obd_dtc_summary(obd.dtc_summary.into());

    let perf = ctrl.perf_hud();
    ui.set_perf_fuel_type(perf.fuel_type.into());
    ui.set_perf_rpm_num(perf.rpm_num.into());
    ui.set_perf_rpm_ratio(perf.rpm_ratio);
    ui.set_perf_shift_urgent(perf.shift_urgent);
    ui.set_perf_shift_reason(perf.shift_reason.into());
    ui.set_perf_speed_num(perf.speed_num.into());
    ui.set_perf_power_hp(perf.power_hp.into());
    ui.set_perf_torque_nm(perf.torque_nm.into());
    ui.set_perf_pedal_pct(perf.pedal_pct.into());
    ui.set_perf_pedal_ratio(perf.pedal_ratio);
    ui.set_perf_coolant_state(perf.coolant_state.into());
    ui.set_perf_coolant_label(perf.coolant_label.into());
    ui.set_perf_g_lon(perf.g_lon);
    ui.set_perf_g_lat(perf.g_lat);
    ui.set_perf_g_total(perf.g_total);
    ui.set_perf_g_total_text(perf.g_total_text.into());
    ui.set_perf_g_peak_text(perf.g_peak_text.into());
    ui.set_perf_g_axes_text(perf.g_axes_text.into());
    ui.set_perf_t_0_50(perf.t_0_50.into());
    ui.set_perf_t_0_100(perf.t_0_100.into());
    ui.set_perf_t_80_120(perf.t_80_120.into());
    ui.set_perf_t_0_50_live(perf.t_0_50_live);
    ui.set_perf_t_0_100_live(perf.t_0_100_live);
    ui.set_perf_t_80_120_live(perf.t_80_120_live);
    ui.set_perf_vmax(perf.vmax.into());
    ui.set_perf_tab(ctrl.perf_tab.clone().into());
    ui.set_perf_instant_l100(perf.instant_l100.into());
    ui.set_perf_avg_l100(perf.avg_l100.into());
    ui.set_perf_range_km(perf.range_km.into());
    ui.set_perf_eco_score(perf.eco_score.into());
    ui.set_perf_trip_active(perf.trip_active);
    ui.set_perf_trip_dist(perf.trip_dist.into());
    ui.set_perf_trip_dur(perf.trip_dur.into());
    ui.set_perf_dpf_soot(perf.dpf_soot.into());
    ui.set_perf_dpf_temp(perf.dpf_temp.into());
    ui.set_perf_rail_bar(perf.rail_bar.into());
    ui.set_perf_egr_pct(perf.egr_pct.into());
    ui.set_perf_lambda_val(perf.lambda_val.into());
    ui.set_perf_oil_temp(perf.oil_temp.into());
    ui.set_perf_dtc_count(perf.dtc_count);
    ui.set_perf_dtc_list(perf.dtc_list.into());
    ui.set_dtc_clear_armed(ctrl.dtc_clear_armed);
    ui.set_diag_feedback(ctrl.diag_feedback.clone().into());
    ui.set_clock_time(ctrl.clock_time().into());

    let flash = ctrl.active_alert_flash();
    ui.set_alert_flash_active(flash.active);
    ui.set_alert_flash_severity(flash.severity.into());
    ui.set_alert_flash_title(flash.title.into());
    ui.set_alert_flash_subtitle(flash.subtitle.into());
}

fn refresh_qr_codes(ui: &DashboardWindow, ctrl: &DashboardController) {
    let wifi_payload = ctrl.wifi_qr_payload();
    if let Some(m) = qr::QrMatrix::encode(&wifi_payload) {
        ui.set_qr_wifi_image(m.to_slint_image(4));
        ui.set_has_qr_wifi(true);
    }
    if let Some(m) = qr::QrMatrix::encode("http://192.168.50.2") {
        ui.set_qr_web_image(m.to_slint_image(5));
        ui.set_has_qr_web(true);
    }
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
        ui.on_reset_perf(move || {
            let c = ctrl.borrow();
            c.reset_perf();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_toggle_fuel_type(move || {
            let c = ctrl.borrow();
            c.toggle_fuel_type();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_set_perf_tab(move |tab| {
            let mut c = ctrl.borrow_mut();
            c.perf_tab = tab.to_string();
            c.dtc_clear_armed = false;
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_diag_read_dtcs(move || {
            let mut c = ctrl.borrow_mut();
            c.dtc_clear_armed = false;
            c.request_obd_post("/api/obd/dtc/read", "Lecture des défauts ECU demandée…");
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_diag_clear_dtcs(move || {
            let mut c = ctrl.borrow_mut();
            if !c.dtc_clear_armed {
                c.dtc_clear_armed = true;
                c.diag_feedback = "Appuyez une 2e fois pour confirmer l'effacement (Mode 04).".into();
            } else {
                c.dtc_clear_armed = false;
                c.request_obd_post("/api/obd/dtc/clear", "Effacement des codes défauts envoyé (Mode 04).");
            }
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_dismiss_alert(move || {
            let mut c = ctrl.borrow_mut();
            c.dismiss_alert_flash();
            refresh(&c, false);
            *dirty.borrow_mut() = true;
        });
    }
    {
        let ctrl = ctrl.clone();
        let dirty = dirty.clone();
        let refresh = refresh.clone();
        ui.on_test_alert_flash(move || {
            let mut c = ctrl.borrow_mut();
            c.trigger_test_flash();
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

fn has_idr_nal(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                if (data[i + 3] & 0x1f) == 5 {
                    return true;
                }
                i += 3;
                continue;
            }
            if i + 4 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                if (data[i + 4] & 0x1f) == 5 {
                    return true;
                }
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// Long-lived zero-latency `ffmpeg` libx264 worker that encodes 800x480 RGB24
/// frames in ~25-35 ms on Raspberry Pi Zero 2 W (vs ~450 ms per fork/exec).
struct PersistentH264Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    chroma: i32,
}

impl PersistentH264Encoder {
    fn spawn(width: u32, height: u32, chroma: i32) -> Result<Self, String> {
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
                "-an",
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-crf",
                "22",
                "-profile:v",
                "high",
                "-threads",
                "1",
                "-x264-params",
                &format!(
                    "keyint=1:bframes=0:rc-lookahead=0:sync-lookahead=0:cabac=1:8x8dct=1:chroma-qp-offset={chroma}:deblock=0,0:repeat-headers=1:slices=1"
                ),
                "-flush_packets",
                "1",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn persistent ffmpeg: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "persistent ffmpeg missing stdin".to_string())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "persistent ffmpeg missing stdout".to_string())?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("h264-rx".into())
            .spawn(move || {
                let mut buf = vec![0u8; 32 * 1024];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn h264 reader thread: {e}"))?;

        Ok(Self {
            child,
            stdin: Some(stdin),
            rx,
            width,
            height,
            chroma,
        })
    }

    fn matches(&self, width: u32, height: u32, chroma: i32) -> bool {
        self.width == width && self.height == height && self.chroma == chroma
    }

    fn encode_raw(&mut self, rgb24: &[u8]) -> Result<Vec<u8>, String> {
        // Drop any unexpected stale bytes before pushing the next frame.
        while self.rx.try_recv().is_ok() {}

        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "persistent ffmpeg stdin closed".to_string())?;
        stdin
            .write_all(rgb24)
            .map_err(|e| format!("persistent ffmpeg write: {e}"))?;
        stdin
            .flush()
            .map_err(|e| format!("persistent ffmpeg flush: {e}"))?;

        let mut raw = Vec::with_capacity(64 * 1024);
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut saw_idr = false;
        while Instant::now() < deadline {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(chunk) => {
                    raw.extend_from_slice(&chunk);
                    if has_idr_nal(&raw) {
                        saw_idr = true;
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err("persistent ffmpeg timeout waiting for IDR".into());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("persistent ffmpeg stdout closed".into());
                }
            }
        }
        if !saw_idr {
            return Err("persistent ffmpeg produced no IDR".into());
        }

        // Drain any remaining pipe chunks belonging to the same IDR access unit.
        let drain_deadline = Instant::now() + Duration::from_millis(90);
        while Instant::now() < drain_deadline {
            match self.rx.recv_timeout(Duration::from_millis(10)) {
                Ok(chunk) => raw.extend_from_slice(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(raw)
    }
}

impl Drop for PersistentH264Encoder {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn encode_h264_oneshot(
    rgb24: &[u8],
    width: u32,
    height: u32,
    chroma: i32,
    target_cfg: &[u8],
) -> Result<Vec<u8>, String> {
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
            "-frames:v",
            "1",
            "-an",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "21",
            "-profile:v",
            "high",
            "-threads",
            "1",
            "-x264-params",
            &format!(
                "keyint=1:bframes=0:cabac=1:8x8dct=1:chroma-qp-offset={chroma}:deblock=0,0:slices=1"
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

    let mut raw_h264 = Vec::with_capacity(64 * 1024);
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

    align_h264(&raw_h264, target_cfg).map_err(|e| format!("h264 align: {e}"))
}

fn encode_h264_aligned(
    rgb24: &[u8],
    width: u32,
    height: u32,
    reference_h264: &Path,
    cache: &mut HashMap<(u32, u32), (Vec<u8>, i32)>,
    encoder: &mut Option<PersistentH264Encoder>,
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

    if !encoder
        .as_ref()
        .is_some_and(|enc| enc.matches(width, height, chroma))
    {
        *encoder = PersistentH264Encoder::spawn(width, height, chroma).ok();
    }

    if let Some(enc) = encoder.as_mut() {
        match enc
            .encode_raw(rgb24)
            .and_then(|raw| align_h264(&raw, &target_cfg).map_err(|e| format!("h264 align: {e}")))
        {
            Ok(aligned) => return Ok(aligned),
            Err(err) => {
                eprintln!("DASHBOARD_PERSIST_ENC_FALLBACK {err}");
                *encoder = None;
            }
        }
    }

    encode_h264_oneshot(rgb24, width, height, chroma, &target_cfg)
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
    refresh_qr_codes(&ui, &ctrl.borrow());
    sync_ui(&ui, &ctrl.borrow(), false);

    let endpoint = paths.runtime.join("input.sock");
    let _ = fs::remove_file(&endpoint);
    let server = UnixDatagram::bind(&endpoint)
        .map_err(|e| format!("bind {}: {e}", endpoint.display()))?;
    server
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(|e| format!("set_read_timeout: {e}"))?;

    let mut pixel_buf = Vec::new();
    let mut cfg_cache: HashMap<(u32, u32), (Vec<u8>, i32)> = HashMap::new();
    let mut encoder: Option<PersistentH264Encoder> = None;
    let mut last_telemetry_refresh = Instant::now();
    let mut last_obd_publish = Instant::now();
    let mut last_obd_mtime = fs::metadata(paths.bridge.join("obd.json"))
        .and_then(|m| m.modified())
        .ok();
    let mut last_logo_mtime = fs::metadata(paths.settings.join("logo.png"))
        .and_then(|m| m.modified())
        .ok();

    let publish = |ui: &DashboardWindow,
                   window: &MinimalSoftwareWindow,
                   ctrl: &mut DashboardController,
                   pixel_buf: &mut Vec<Rgb8Pixel>,
                   cfg_cache: &mut HashMap<(u32, u32), (Vec<u8>, i32)>,
                   encoder: &mut Option<PersistentH264Encoder>| {
        let (w, h) = ctrl.dimensions();
        ctrl.size = (w, h);
        let ref_h264 = ctrl.paths.assets.join(format!("attente-{w}x{h}.h264"));

        // 1. Render & publish active menu screen
        sync_ui(ui, ctrl, false);
        let rgb = render_rgb24(window, w, h, pixel_buf);
        match encode_h264_aligned(&rgb, w, h, &ref_h264, cfg_cache, encoder) {
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
            match encode_h264_aligned(&wait_rgb, w, h, &ref_h264, cfg_cache, encoder) {
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
            let _ = render_rgb24(window, w, h, pixel_buf);
        }
    };

    publish(
        &ui,
        &window,
        &mut ctrl.borrow_mut(),
        &mut pixel_buf,
        &mut cfg_cache,
        &mut encoder,
    );
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

        // Live dynamic refresh (~12-15 FPS cap on Perf, ~0.8 FPS on Home) whenever obd.json updates
        let page_now = ctrl.borrow().active_page.clone();
        let min_interval = if page_now == "perf" {
            Duration::from_millis(60)
        } else if page_now == "home" {
            Duration::from_millis(1200)
        } else {
            Duration::from_secs(3600)
        };
        if (page_now == "perf" || page_now == "home") && last_obd_publish.elapsed() >= min_interval {
            let cur_obd_mtime = fs::metadata(paths.bridge.join("obd.json"))
                .and_then(|m| m.modified())
                .ok();
            if cur_obd_mtime != last_obd_mtime {
                last_obd_mtime = cur_obd_mtime;
                last_obd_publish = Instant::now();
                need_publish = true;
            }
        }

        // Continuous background alert-flash check (works even while CarPlay is active!)
        let alert_ctrl_ev = {
            let (_, ev) = ctrl.borrow().poll_alert_flash();
            ev
        };
        if let Some(ev) = alert_ctrl_ev {
            // Render the alert card (or clean restored screen) BEFORE signalling aa-phone!
            publish(
                &ui,
                &window,
                &mut ctrl.borrow_mut(),
                &mut pixel_buf,
                &mut cfg_cache,
                &mut encoder,
            );
            *dirty.borrow_mut() = false;
            need_publish = false;
            ctrl.borrow_mut().shell_action(ev);
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
            publish(
                &ui,
                &window,
                &mut ctrl.borrow_mut(),
                &mut pixel_buf,
                &mut cfg_cache,
                &mut encoder,
            );
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
/// Polls `/tmp/carfluxoid-preview-settings/obd.json` & `video.sock.display` at 10 Hz
/// so `scripts/obd-sim.py` can drive the real Uconnect OBD Sport screen interactively.
#[cfg(feature = "preview")]
pub fn run_preview(logo_path: Option<&Path>) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("carfluxoid-preview-settings");
    let _ = fs::create_dir_all(&tmp);
    let disp_path = tmp.join("video.sock.display");
    if !disp_path.exists() {
        // Default to Fiat Tipo Uconnect 7" geometry with safe margins (0, 32).
        let _ = fs::write(&disp_path, "800 480 0 32\n");
    }
    let paths = Paths {
        settings: tmp.clone(),
        assets: Path::new("assets/waiting").to_path_buf(),
        runtime: tmp.clone(),
        bridge: tmp,
    };
    let mut initial_ctrl = DashboardController::new(paths, true);
    initial_ctrl.active_page = "home".into();
    let ctrl = Rc::new(RefCell::new(initial_ctrl));
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
    refresh_qr_codes(&ui, &ctrl.borrow());
    sync_ui(&ui, &ctrl.borrow(), false);
    ui.set_oem_name("Fiat".into());

    let ui_weak = ui.as_weak();
    let ctrl_poll = ctrl.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            if let Some(w) = ui_weak.upgrade() {
                let c = ctrl_poll.borrow();
                let _ = c.poll_alert_flash();
                sync_ui(&w, &c, false);
                w.set_oem_name("Fiat".into());
            }
        },
    );

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

#[cfg(test)]
mod encoder_tests {
    use super::*;

    #[test]
    fn persistent_encoder_produces_valid_aligned_frames() {
        let ref_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../assets/waiting/attente-800x480.h264");
        if !ref_path.is_file() || Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let mut cache = HashMap::new();
        let mut enc: Option<PersistentH264Encoder> = None;
        let mut rgb = vec![0u8; 800 * 480 * 3];
        for i in 0..5u8 {
            for px in rgb.chunks_exact_mut(3) {
                px[0] = i.wrapping_mul(40);
                px[1] = 24;
                px[2] = 60;
            }
            let h264 = encode_h264_aligned(&rgb, 800, 480, &ref_path, &mut cache, &mut enc)
                .expect("encode aligned frame");
            let idrs = crate::h264_align::iter_nals(&h264)
                .into_iter()
                .filter(|n| !n.is_empty() && (n[0] & 31) == 5)
                .count();
            assert_eq!(idrs, 2, "expected 2 aligned IDRs per frame (idr_pic_id 0 & 1)");
            assert!(enc.is_some(), "persistent encoder should stay alive across frames");
        }
    }
}
