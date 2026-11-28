//! Dashboard settings persistence, live vehicle/Pi telemetry, and CarPlay lifecycle control.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    net::TcpStream,
    os::unix::{fs::MetadataExt, net::UnixDatagram},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const VALID_SIZES: [(u32, u32); 3] = [(800, 480), (1280, 720), (1920, 1080)];

#[derive(Clone, Debug)]
pub struct Paths {
    pub settings: PathBuf,
    pub assets: PathBuf,
    pub runtime: PathBuf,
    pub bridge: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            settings: PathBuf::from("/var/lib/carfluxoid/settings"),
            assets: PathBuf::from("/opt/carfluxoid/assets"),
            runtime: PathBuf::from("/run/carfluxoid-ui"),
            bridge: PathBuf::from("/run/carfluxoid"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Telemetry {
    pub version: String,
    pub build: String,
    pub cpu_temp: String,
    pub gps_status: String,
    pub night_status: String,
    pub night_is_dark: bool,
    pub display_status: String,
}

#[derive(Clone, Debug)]
pub struct ObdHud {
    pub connected: bool,
    pub dpf_regen: bool,
    pub status: String,
    pub rpm: String,
    pub rpm_ratio: f32,
    pub speed: String,
    pub gear: String,
    pub shift_hint: String, // "up" | "down" | ""
    pub power_torque: String,
    pub boost: String,
    pub boost_ratio: f32,
    pub coolant: String,
    pub fuel: String,
    pub fuel_ratio: f32,
    pub battery: String,
    pub consumption: String,
    pub rail_egr: String,
    pub dtc_summary: String,
}

#[derive(Clone, Debug)]
pub struct PerfHud {
    pub fuel_type: String,     // "diesel" | "petrol"
    pub rpm_num: String,
    pub rpm_ratio: f32,        // 0..1 against 5000 (diesel) or 7000 (petrol)
    pub shift_urgent: bool,    // true when near redline or over cold-engine limit
    pub shift_reason: String,  // "FROID" | "ÉCO" | "SPORT" | "BAS RÉGIME" | ""
    pub speed_num: String,
    pub power_hp: String,
    pub torque_nm: String,
    pub pedal_pct: String,
    pub pedal_ratio: f32,
    pub coolant_state: String, // "cold" | "ready" | "hot" | ""
    pub coolant_label: String,
    pub g_lon: f32,
    pub g_lat: f32,
    pub g_total: f32,
    pub g_total_text: String,
    pub g_peak_text: String,
    pub g_axes_text: String,
    pub t_0_50: String,
    pub t_0_100: String,
    pub t_80_120: String,
    pub t_0_50_live: bool,
    pub t_0_100_live: bool,
    pub t_80_120_live: bool,
    pub vmax: String,
    // Voyage / Eco & Sante / Diag telemetry
    pub instant_l100: String,
    pub avg_l100: String,
    pub range_km: String,
    pub eco_score: String,
    pub trip_active: bool,
    pub trip_dist: String,
    pub trip_dur: String,
    pub dpf_soot: String,
    pub dpf_temp: String,
    pub rail_bar: String,
    pub egr_pct: String,
    pub lambda_val: String,
    pub oil_temp: String,
    pub dtc_count: i32,
    pub dtc_list: String,
}

#[derive(Clone, Debug, Default)]
pub struct AlertFlash {
    pub active: bool,
    pub severity: String, // "critical" | "warning" | "info"
    pub title: String,
    pub subtitle: String,
}

#[derive(Debug, Default)]
struct AlertMonitor {
    active: Option<(AlertFlash, Instant)>,
    was_dpf_regen: bool,
    warm_armed: bool,
    warm_announced: bool,
    last_overheat_at: Option<Instant>,
    low_batt_since: Option<Instant>,
    last_low_batt_at: Option<Instant>,
    known_dtcs: BTreeSet<String>,
    dtc_ready: bool,
    clock_cache: Option<(String, Instant)>,
}

#[derive(Debug)]
struct PerfTracker {
    last_sample_at: Option<Instant>,
    last_speed_kmh: f64,
    smooth_g_lon: f64,
    smooth_g_lat: f64,
    g_peak: f64,
    vmax_kmh: f64,
    launch_armed: bool,
    launch_start: Option<Instant>,
    run_0_50_done: bool,
    best_0_50: Option<f64>,
    best_0_100: Option<f64>,
    reprise_armed: bool,
    reprise_start: Option<Instant>,
    best_80_120: Option<f64>,
}

impl Default for PerfTracker {
    fn default() -> Self {
        Self {
            last_sample_at: None,
            last_speed_kmh: 0.0,
            smooth_g_lon: 0.0,
            smooth_g_lat: 0.0,
            g_peak: 0.0,
            vmax_kmh: 0.0,
            launch_armed: true,
            launch_start: None,
            run_0_50_done: false,
            best_0_50: None,
            best_0_100: None,
            reprise_armed: true,
            reprise_start: None,
            best_80_120: None,
        }
    }
}

fn format_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::with_capacity(s.len() + 2);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

fn json_num(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    if rest.starts_with("null") {
        return None;
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}

fn json_str<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn json_bool(text: &str, key: &str) -> bool {
    let needle = format!("\"{key}\":");
    let Some(pos) = text.find(&needle) else {
        return false;
    };
    text[pos + needle.len()..].trim_start().starts_with("true")
}

fn extract_trip_num(text: &str, key: &str) -> Option<f64> {
    let start = text.find("\"active_trip\":")?;
    let sub = &text[start..];
    let end = sub.find('}')?;
    json_num(&sub[..=end], key)
}

fn extract_dtc_codes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(pos) = text.find("\"dtcs\":") else {
        return out;
    };
    let rest = &text[pos + 7..];
    let Some(lb) = rest.find('[') else { return out; };
    let after = &rest[lb + 1..];
    let Some(rb) = after.find(']') else { return out; };
    let mut slice = &after[..rb];
    while let Some(cpos) = slice.find("\"code\":") {
        let after_code = slice[cpos + 7..].trim_start();
        if let Some(stripped) = after_code.strip_prefix('"') {
            if let Some(q) = stripped.find('"') {
                let code = stripped[..q].trim().to_ascii_uppercase();
                if !code.is_empty() && !out.contains(&code) {
                    out.push(code);
                }
                slice = &stripped[q + 1..];
                continue;
            }
        }
        break;
    }
    out
}

fn describe_dtc_fr(code: &str) -> &'static str {
    match code {
        "P0087" => "Pression rail carburant trop basse",
        "P0088" => "Pression rail carburant trop élevée",
        "P0101" => "Débitmètre d'air (MAF) incohérent",
        "P0115" | "P0116" => "Capteur température eau moteur",
        "P0201" => "Injecteur cylindre 1",
        "P0202" => "Injecteur cylindre 2",
        "P0203" => "Injecteur cylindre 3",
        "P0204" => "Injecteur cylindre 4",
        "P0234" => "Surpression turbo détectée",
        "P0299" => "Pression turbo insuffisante",
        "P0380" | "P0670" => "Circuit bougies de préchauffage",
        "P0401" => "Débit EGR insuffisant (vanne encrassée)",
        "P0402" => "Débit EGR excessif",
        "P0403" | "P0404" => "Commande / blocage vanne EGR",
        "P0562" => "Tension batterie trop basse",
        "P2002" => "Efficacité FAP sous le seuil",
        "P242F" => "Filtre à particules (FAP) colmaté",
        "P2452" | "P2453" | "P2454" | "P2455" => "Capteur pression différentielle FAP",
        "P2463" => "Accumulation suie FAP excessive",
        "P2563" => "Capteur position turbo VGT",
        _ if code.starts_with("U0") || code.starts_with("U1") => "Défaut communication bus CAN",
        _ if code.starts_with("P040") => "Système EGR",
        _ if code.starts_with("P24") => "Système FAP / échappement",
        _ => "Code défaut calculateur moteur",
    }
}

pub struct DashboardController {
    pub paths: Paths,
    pub active_page: String,
    pub perf_tab: String, // "corsa" | "voyage" | "sante"
    pub dtc_clear_armed: bool,
    pub diag_feedback: String,
    pub edit_target: String,
    pub draft: String,
    pub shift: bool,
    pub notice: String,
    pub size: (u32, u32),
    pub waiting_key: Option<((u32, u32), String)>,
    carplay_job: Option<Child>,
    auto_started_key: Option<(u64, i64, i64)>,
    auto_start_pending: Option<((u64, i64, i64), Instant)>,
    control_sock: Option<UnixDatagram>,
    perf: RefCell<PerfTracker>,
    alerts: RefCell<AlertMonitor>,
    pub dry_run: bool,
}

impl DashboardController {
    pub fn new(paths: Paths, dry_run: bool) -> Self {
        let control_sock = if dry_run {
            None
        } else {
            UnixDatagram::unbound().ok()
        };
        Self {
            paths,
            active_page: "home".into(),
            perf_tab: "corsa".into(),
            dtc_clear_armed: false,
            diag_feedback: String::new(),
            edit_target: "wifi".into(),
            draft: String::new(),
            shift: false,
            notice: String::new(),
            size: (800, 480),
            waiting_key: None,
            carplay_job: None,
            auto_started_key: None,
            auto_start_pending: None,
            control_sock,
            perf: RefCell::new(PerfTracker::default()),
            alerts: RefCell::new(AlertMonitor::default()),
            dry_run,
        }
    }

    pub fn reset_perf(&self) {
        *self.perf.borrow_mut() = PerfTracker::default();
    }

    pub fn fuel_type(&self) -> String {
        match Self::read_trimmed(&self.paths.settings.join("fuel-type.txt")).as_deref() {
            Some("petrol") => "petrol".into(),
            _ => "diesel".into(),
        }
    }

    pub fn toggle_fuel_type(&self) {
        let next = if self.fuel_type() == "diesel" { "petrol" } else { "diesel" };
        let _ = atomic_write(&self.paths.settings.join("fuel-type.txt"), format!("{next}\n").as_bytes(), false);
    }

    fn compute_shift(
        fuel_type: &str,
        gear_num: Option<u8>,
        rpm_val: Option<f64>,
        speed_val: Option<f64>,
        pedal_val: Option<f64>,
        coolant_val: Option<f64>,
    ) -> (String, bool, String) {
        let (Some(g), Some(rpm)) = (gear_num, rpm_val) else {
            return (String::new(), false, String::new());
        };
        let spd = speed_val.unwrap_or(0.0);
        let pedal = pedal_val.unwrap_or(20.0);
        let cold = coolant_val.is_some_and(|c| c < 75.0);
        let is_diesel = fuel_type != "petrol";

        let (cold_up, eco_up, sport_up, urgent_up, down_rpm) = if is_diesel {
            // Fiat MultiJet Diesel: couple maxi dès 1750 tr, protège moteur froid & bi-masse
            (2000.0, 2150.0, 3200.0, 3800.0, 1300.0)
        } else {
            // Essence: plage plus large, zone rouge ~6500 tr
            (2500.0, 2800.0, 5500.0, 6100.0, 1500.0)
        };

        if (1..=5).contains(&g) {
            if cold && rpm >= cold_up {
                return ("up".into(), rpm >= cold_up + 300.0, "MOTEUR FROID".into());
            }
            let target_up = if pedal >= 65.0 { sport_up } else { eco_up };
            if rpm >= target_up {
                let urgent = rpm >= urgent_up;
                let reason = if urgent {
                    "ZONE ROUGE"
                } else if pedal >= 65.0 {
                    "SPORT"
                } else {
                    "ÉCO"
                };
                return ("up".into(), urgent, reason.into());
            }
        }
        if (2..=6).contains(&g) && spd >= 12.0 && rpm <= down_rpm {
            return ("down".into(), false, "SOUS-RÉGIME".into());
        }
        (String::new(), false, String::new())
    }

    pub fn read_trimmed(path: &Path) -> Option<String> {
        fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn dimensions(&self) -> (u32, u32) {
        if let Some(raw) = Self::read_trimmed(&self.paths.bridge.join("video.sock.display")) {
            let mut parts = raw.split_whitespace();
            if let (Some(w), Some(h)) = (parts.next(), parts.next()) {
                if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                    if VALID_SIZES.contains(&(w, h)) {
                        return (w, h);
                    }
                }
            }
        }
        (800, 480)
    }

    /// Safe-area insets (per side, in logical 800x480 px) matching CarPlay's CP_VIEW_AREA margins.
    /// Reads columns 3 & 4 (margin_w, margin_h) from video.sock.display (e.g. "800 480 0 32" -> (0, 16)).
    pub fn safe_insets(&self) -> (f32, f32) {
        if let Some(raw) = Self::read_trimmed(&self.paths.bridge.join("video.sock.display")) {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            if parts.len() >= 4 {
                if let (Ok(w), Ok(h), Ok(mw), Ok(mh)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                    parts[3].parse::<f32>(),
                ) {
                    if w > 0.0 && h > 0.0 {
                        let ix = ((mw * 0.5) * (800.0 / w)).clamp(0.0, 100.0);
                        let iy = ((mh * 0.5) * (480.0 / h)).clamp(0.0, 80.0);
                        return (ix, iy);
                    }
                }
            }
        }
        (0.0, 0.0)
    }

    pub fn obd_hud(&self) -> ObdHud {
        let Some(raw) = Self::read_trimmed(&self.paths.bridge.join("obd.json")) else {
            return ObdHud {
                connected: false,
                dpf_regen: false,
                status: "Appairez via l'app CarFluxoid".into(),
                rpm: "—".into(),
                rpm_ratio: 0.0,
                speed: "—".into(),
                gear: "—".into(),
                shift_hint: String::new(),
                power_torque: "—".into(),
                boost: "—".into(),
                boost_ratio: 0.0,
                coolant: "—".into(),
                fuel: "—".into(),
                fuel_ratio: 0.0,
                battery: "—".into(),
                consumption: "—".into(),
                rail_egr: "—".into(),
                dtc_summary: "Aucun défaut".into(),
            };
        };
        let connected = json_bool(&raw, "connected");
        let dpf_regen = json_bool(&raw, "dpf_regen_active");
        let state = json_str(&raw, "state").unwrap_or("disconnected");
        let status = if dpf_regen {
            "REGEN FAP EN COURS — ne coupez pas".into()
        } else if connected {
            json_str(&raw, "adapter_name")
                .unwrap_or("Vgate connecté")
                .to_string()
        } else if state == "connecting" || state == "reconnecting" {
            "Connexion au boîtier…".into()
        } else {
            "Appairez via l'app CarFluxoid".into()
        };
        let rpm_val = json_num(&raw, "rpm");
        let speed_val = json_num(&raw, "speed_kmh");
        let boost_val = json_num(&raw, "boost_bar");
        let fuel_val = json_num(&raw, "fuel_level_pct");
        let hp_val = json_num(&raw, "power_hp");
        let nm_val = json_num(&raw, "torque_nm");
        let rail_val = json_num(&raw, "rail_bar");
        let egr_val = json_num(&raw, "egr_pct");
        let lam_val = json_num(&raw, "lambda");
        let dpf_temp = json_num(&raw, "dpf_temp_c");
        let gear_str = json_str(&raw, "gear").unwrap_or("—").to_string();
        let gear_num = gear_str.parse::<u8>().ok();
        let pedal_val = json_num(&raw, "pedal_pct").or_else(|| json_num(&raw, "throttle_pct"));
        let coolant_val = json_num(&raw, "coolant_c");
        let ft = self.fuel_type();
        let max_rpm = if ft == "petrol" { 7000.0 } else { 5000.0 };
        let (shift_hint, _, _) = Self::compute_shift(&ft, gear_num, rpm_val, speed_val, pedal_val, coolant_val);

        let power_torque = match (hp_val, nm_val) {
            (Some(hp), Some(nm)) => format!("{:.0} ch · {:.0} Nm", hp, nm),
            (Some(hp), None) => format!("{:.0} ch", hp),
            _ => "—".into(),
        };
        let rail_egr = match (rail_val, egr_val) {
            (Some(r), Some(e)) => format!("Rail {:.0} bar · EGR {:.0}%", r, e),
            (Some(r), None) => format!("Rail {:.0} bar", r),
            (None, Some(e)) => format!("EGR {:.0} %", e),
            (None, None) => "MultiJet OBD-II".into(),
        };
        let dtc_count = raw
            .find("\"dtcs\":[")
            .map(|i| {
                let slice = &raw[i + 8..];
                let end = slice.find(']').unwrap_or(0);
                slice[..end].matches("\"code\"").count()
            })
            .unwrap_or(0);
        ObdHud {
            connected,
            dpf_regen,
            status,
            rpm: rpm_val
                .map(|v| format!("{:.0} tr/min", v))
                .unwrap_or_else(|| "—".into()),
            rpm_ratio: rpm_val
                .map(|v| (v as f32 / max_rpm as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            speed: speed_val
                .map(|v| format!("{:.0} km/h", v))
                .unwrap_or_else(|| "—".into()),
            gear: gear_str,
            shift_hint,
            power_torque,
            boost: boost_val
                .map(|v| format!("{:+.2} bar", v))
                .unwrap_or_else(|| "—".into()),
            boost_ratio: boost_val
                .map(|v| ((v as f32 + 0.2) / 1.8).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            coolant: json_num(&raw, "coolant_c")
                .map(|v| format!("{:.0} °C", v))
                .unwrap_or_else(|| "—".into()),
            fuel: fuel_val
                .map(|v| format!("{:.0} %", v))
                .unwrap_or_else(|| "—".into()),
            fuel_ratio: fuel_val
                .map(|v| (v as f32 / 100.0).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            battery: json_num(&raw, "battery_v")
                .map(|v| format!("{:.1} V", v))
                .unwrap_or_else(|| "—".into()),
            consumption: json_num(&raw, "fuel_l_per_100km")
                .map(|v| format!("{:.1} L/100", v))
                .or_else(|| json_num(&raw, "fuel_rate_lph").map(|v| format!("{:.1} L/h", v)))
                .unwrap_or_else(|| "—".into()),
            rail_egr,
            dtc_summary: if dtc_count == 0 {
                match (dpf_temp, lam_val) {
                    (Some(t), Some(l)) => format!("FAP {:.0}°C · λ {:.2}", t, l),
                    (Some(t), None) => format!("FAP {:.0}°C · 0 défaut", t),
                    (None, Some(l)) => format!("λ {:.2} · 0 défaut", l),
                    (None, None) => "Aucun défaut".into(),
                }
            } else {
                format!("{dtc_count} défaut(s)")
            },
        }
    }

    pub fn perf_hud(&self) -> PerfHud {
        let raw = Self::read_trimmed(&self.paths.bridge.join("obd.json")).unwrap_or_default();
        let connected = json_bool(&raw, "connected");
        let rpm_val = if connected { json_num(&raw, "rpm") } else { None };
        let speed_val = if connected { json_num(&raw, "speed_kmh") } else { None };
        let hp_val = if connected { json_num(&raw, "power_hp") } else { None };
        let nm_val = if connected { json_num(&raw, "torque_nm") } else { None };
        let pedal_val = if connected {
            json_num(&raw, "pedal_pct").or_else(|| json_num(&raw, "throttle_pct"))
        } else {
            None
        };
        let coolant_val = if connected { json_num(&raw, "coolant_c") } else { None };
        let json_g_lon = if connected { json_num(&raw, "g_lon") } else { None };
        let json_g_lat = if connected { json_num(&raw, "g_lat") } else { None };

        let mut p = self.perf.borrow_mut();
        let now = Instant::now();
        let spd = speed_val.unwrap_or(0.0).max(0.0);

        // Longitudinal & lateral G-force (direct from OBD/sim JSON, or derived from speed delta)
        if let Some(gl) = json_g_lon {
            p.smooth_g_lon = p.smooth_g_lon * 0.35 + gl.clamp(-1.5, 1.5) * 0.65;
        } else if let Some(prev_t) = p.last_sample_at {
            let dt = now.saturating_duration_since(prev_t).as_secs_f64();
            if dt >= 0.06 && dt <= 2.5 {
                let dv_ms = (spd - p.last_speed_kmh) / 3.6;
                let inst_g = (dv_ms / (dt * 9.81)).clamp(-1.3, 1.3);
                p.smooth_g_lon = p.smooth_g_lon * 0.55 + inst_g * 0.45;
            } else if dt > 2.5 {
                p.smooth_g_lon *= 0.5;
            }
        }
        if let Some(gt) = json_g_lat {
            p.smooth_g_lat = p.smooth_g_lat * 0.35 + gt.clamp(-1.5, 1.5) * 0.65;
        } else {
            p.smooth_g_lat *= 0.85;
        }
        if !connected || (spd < 0.5 && json_g_lon.is_none()) {
            p.smooth_g_lon *= 0.6;
            p.smooth_g_lat *= 0.6;
        }
        if p.smooth_g_lon.abs() < 0.015 { p.smooth_g_lon = 0.0; }
        if p.smooth_g_lat.abs() < 0.015 { p.smooth_g_lat = 0.0; }

        p.last_sample_at = Some(now);
        p.last_speed_kmh = spd;

        let g_total = (p.smooth_g_lon * p.smooth_g_lon + p.smooth_g_lat * p.smooth_g_lat).sqrt();
        if g_total > p.g_peak {
            p.g_peak = g_total;
        }
        if spd > p.vmax_kmh {
            p.vmax_kmh = spd;
        }

        // Launch timers: 0-50 & 0-100 km/h
        if spd <= 1.5 {
            p.launch_armed = true;
            p.launch_start = None;
            p.run_0_50_done = false;
        } else if p.launch_armed && p.launch_start.is_none() && spd >= 3.0 {
            p.launch_start = Some(now);
            p.run_0_50_done = false;
        } else if let Some(t0) = p.launch_start {
            let el = now.saturating_duration_since(t0).as_secs_f64();
            if el > 30.0 {
                p.launch_start = None;
                p.launch_armed = false;
            } else {
                if !p.run_0_50_done && spd >= 50.0 {
                    p.run_0_50_done = true;
                    if el >= 1.2 && p.best_0_50.is_none_or(|b| el < b) {
                        p.best_0_50 = Some(el);
                    }
                }
                if spd >= 100.0 {
                    if el >= 3.5 && p.best_0_100.is_none_or(|b| el < b) {
                        p.best_0_100 = Some(el);
                    }
                    p.launch_start = None;
                    p.launch_armed = false;
                }
            }
        }

        // Reprise timer: 80-120 km/h
        if spd <= 78.0 {
            p.reprise_armed = true;
            p.reprise_start = None;
        } else if p.reprise_armed && p.reprise_start.is_none() && spd >= 80.0 && spd < 88.0 {
            p.reprise_start = Some(now);
        } else if let Some(t0) = p.reprise_start {
            let el = now.saturating_duration_since(t0).as_secs_f64();
            if el > 30.0 {
                p.reprise_start = None;
                p.reprise_armed = false;
            } else if spd >= 120.0 {
                if el >= 2.0 && p.best_80_120.is_none_or(|b| el < b) {
                    p.best_80_120 = Some(el);
                }
                p.reprise_start = None;
                p.reprise_armed = false;
            }
        }

        let launch_el = p.launch_start.map(|t0| now.saturating_duration_since(t0).as_secs_f64());
        let reprise_el = p.reprise_start.map(|t0| now.saturating_duration_since(t0).as_secs_f64());

        let t_0_50_live = launch_el.is_some() && !p.run_0_50_done;
        let t_0_100_live = launch_el.is_some();
        let t_80_120_live = reprise_el.is_some();

        let t_0_50 = if t_0_50_live {
            format!("{:.1} s", launch_el.unwrap_or(0.0))
        } else if let Some(b) = p.best_0_50 {
            format!("{:.1} s", b)
        } else if p.launch_armed && spd <= 1.5 {
            "Prêt".into()
        } else {
            "—".into()
        };

        let t_0_100 = if t_0_100_live {
            format!("{:.1} s", launch_el.unwrap_or(0.0))
        } else if let Some(b) = p.best_0_100 {
            format!("{:.1} s", b)
        } else if p.launch_armed && spd <= 1.5 {
            "Prêt".into()
        } else {
            "—".into()
        };

        let t_80_120 = if let Some(el) = reprise_el {
            format!("{:.1} s", el)
        } else if let Some(b) = p.best_80_120 {
            format!("{:.1} s", b)
        } else {
            "Auto".into()
        };

        let (coolant_state, coolant_label) = match coolant_val {
            Some(c) if c >= 100.0 => ("hot".into(), format!("{:.0} °C · CHAUD", c)),
            Some(c) if c >= 75.0 => ("ready".into(), format!("{:.0} °C · PRÊT", c)),
            Some(c) => ("cold".into(), format!("{:.0} °C · FROID", c)),
            None => (String::new(), "Eau —".into()),
        };

        let ft = self.fuel_type();
        let max_rpm = if ft == "petrol" { 7000.0 } else { 5000.0 };
        let gear_num = json_str(&raw, "gear").and_then(|g| g.parse::<u8>().ok());
        let (_, shift_urgent, shift_reason) = Self::compute_shift(
            &ft,
            gear_num,
            rpm_val,
            speed_val,
            pedal_val,
            coolant_val,
        );

        PerfHud {
            fuel_type: ft,
            rpm_num: rpm_val
                .map(|v| format_thousands(v.round() as i64))
                .unwrap_or_else(|| "—".into()),
            rpm_ratio: rpm_val
                .map(|v| (v as f32 / max_rpm as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            shift_urgent,
            shift_reason,
            speed_num: speed_val
                .map(|v| format!("{:.0}", v))
                .unwrap_or_else(|| "—".into()),
            power_hp: hp_val
                .map(|v| format!("{:.0} ch", v))
                .unwrap_or_else(|| "— ch".into()),
            torque_nm: nm_val
                .map(|v| format!("{:.0} Nm", v))
                .unwrap_or_else(|| "— Nm".into()),
            pedal_pct: pedal_val
                .map(|v| format!("{:.0} %", v))
                .unwrap_or_else(|| "—".into()),
            pedal_ratio: pedal_val
                .map(|v| (v as f32 / 100.0).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            coolant_state,
            coolant_label,
            g_lon: p.smooth_g_lon as f32,
            g_lat: p.smooth_g_lat as f32,
            g_total: g_total as f32,
            g_total_text: format!("{:.2} G", g_total),
            g_peak_text: format!("{:.2} G", p.g_peak),
            g_axes_text: format!("Lon {:+.2} · Lat {:+.2}", p.smooth_g_lon, p.smooth_g_lat),
            t_0_50,
            t_0_100,
            t_80_120,
            t_0_50_live,
            t_0_100_live,
            t_80_120_live,
            vmax: if p.vmax_kmh >= 1.0 {
                format!("{:.0} km/h", p.vmax_kmh)
            } else {
                "—".into()
            },
            instant_l100: if connected {
                json_num(&raw, "fuel_l_per_100km")
                    .map(|v| format!("{:.1}", v))
                    .or_else(|| json_num(&raw, "fuel_rate_lph").map(|v| format!("{:.1}", v)))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            avg_l100: extract_trip_num(&raw, "avg_l_per_100km")
                .map(|v| format!("{:.1} L/100", v))
                .unwrap_or_else(|| "—".into()),
            range_km: if connected {
                json_num(&raw, "fuel_level_pct")
                    .filter(|v| *v > 0.5)
                    .map(|pct| {
                        let avg = extract_trip_num(&raw, "avg_l_per_100km").unwrap_or(5.4).max(3.5);
                        let liters = 50.0 * (pct / 100.0);
                        format!("~{:.0} km", (liters / avg) * 100.0)
                    })
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            eco_score: extract_trip_num(&raw, "eco_score")
                .map(|v| format!("{:.0} / 100", v))
                .unwrap_or_else(|| "—".into()),
            trip_active: raw.contains("\"active_trip\":{") || raw.contains("\"active_trip\": {"),
            trip_dist: extract_trip_num(&raw, "distance_km")
                .map(|v| format!("{:.1} km", v))
                .unwrap_or_else(|| "0.0 km".into()),
            trip_dur: extract_trip_num(&raw, "duration_s")
                .map(|s| {
                    let mins = (s as u64) / 60;
                    if mins >= 60 {
                        format!("{}h{:02}", mins / 60, mins % 60)
                    } else {
                        format!("{mins} min")
                    }
                })
                .unwrap_or_else(|| "0 min".into()),
            dpf_soot: if connected {
                json_num(&raw, "dpf_soot_pct")
                    .map(|v| format!("{:.0} %", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            dpf_temp: if connected {
                json_num(&raw, "dpf_temp_c")
                    .map(|v| format!("{:.0} °C", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            rail_bar: if connected {
                json_num(&raw, "rail_bar")
                    .map(|v| format!("{:.0} bar", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            egr_pct: if connected {
                json_num(&raw, "egr_pct")
                    .map(|v| format!("{:.0} %", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            lambda_val: if connected {
                json_num(&raw, "lambda")
                    .map(|v| format!("{:.2}", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            oil_temp: if connected {
                json_num(&raw, "oil_c")
                    .map(|v| format!("{:.0} °C", v))
                    .unwrap_or_else(|| "—".into())
            } else {
                "—".into()
            },
            dtc_count: extract_dtc_codes(&raw).len() as i32,
            dtc_list: {
                let codes = extract_dtc_codes(&raw);
                if codes.is_empty() {
                    "Aucun code défaut enregistré · Moteur sain".into()
                } else {
                    codes
                        .into_iter()
                        .take(3)
                        .map(|c| format!("{c} · {}", describe_dtc_fr(&c)))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            },
        }
    }

    pub fn clock_time(&self) -> String {
        let mut am = self.alerts.borrow_mut();
        if let Some((ref cached, at)) = am.clock_cache {
            if at.elapsed() < Duration::from_secs(10) {
                return cached.clone();
            }
        }
        let formatted = Command::new("date")
            .arg("+%H:%M")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 5 && s.contains(':'))
            .unwrap_or_else(|| "--:--".into());
        am.clock_cache = Some((formatted.clone(), Instant::now()));
        formatted
    }

    /// Reads real live Wi-Fi AP SSID & WPA2 passphrase (from /run/hostapd.conf or /data/radio.env)
    /// to build a standard scannable iPhone/Android Wi-Fi QR code payload.
    pub fn wifi_qr_payload(&self) -> String {
        let mut ssid = self.radio_name("wifi");
        let mut pass = String::new();
        if let Ok(conf) = fs::read_to_string("/run/hostapd.conf") {
            for line in conf.lines() {
                if let Some(v) = line.strip_prefix("ssid=") {
                    let s = v.trim();
                    if !s.is_empty() { ssid = s.to_string(); }
                } else if let Some(v) = line.strip_prefix("wpa_passphrase=") {
                    pass = v.trim().to_string();
                }
            }
        }
        if pass.is_empty() {
            if let Ok(env) = fs::read_to_string("/data/radio.env") {
                for line in env.lines() {
                    if let Some(v) = line.strip_prefix("AA_WIFI_PASSWORD=") {
                        pass = v.trim().to_string();
                    }
                }
            }
        }
        if pass.is_empty() {
            pass = "c@rfluxoid".into();
        }
        let esc = |s: &str| s.replace('\\', "\\\\").replace(';', "\\;").replace(':', "\\:");
        format!("WIFI:T:WPA;S:{};P:{};;", esc(&ssid), esc(&pass))
    }

    /// Evaluates live OBD telemetry for critical CarPlay alert flashes (4.5 s overlay).
    /// Returns `(current_flash, optional_aa_control_event: Some(9)=SHOW or Some(10)=HIDE)`.
    pub fn poll_alert_flash(&self) -> (AlertFlash, Option<u8>) {
        let mut am = self.alerts.borrow_mut();
        let now = Instant::now();
        let mut ctrl_event: Option<u8> = None;

        // Auto-expire active alert flash after 4.5 seconds
        if let Some((_, started)) = am.active {
            if started.elapsed() >= Duration::from_millis(4500) {
                am.active = None;
                ctrl_event = Some(10); // ALERT_FLASH_HIDE -> return to CarPlay
            }
        }

        // Check manual test flash trigger written by POST /api/alert/test (works even with real Vgate or CarPlay active!)
        let trigger_path = self.paths.bridge.join("alert-test.txt");
        if let Ok(text) = fs::read_to_string(&trigger_path) {
            let _ = fs::remove_file(&trigger_path);
            let mut parts = text.trim().splitn(3, '|');
            let severity = parts.next().unwrap_or("critical").to_string();
            let title = parts.next().unwrap_or("RÉGÉNÉRATION FAP EN COURS").to_string();
            let subtitle = parts
                .next()
                .unwrap_or("Échappement 624 °C · Ne coupez pas le moteur !")
                .to_string();
            let flash = AlertFlash {
                active: true,
                severity,
                title,
                subtitle,
            };
            am.active = Some((flash.clone(), now));
            return (flash, Some(9));
        }

        let raw = Self::read_trimmed(&self.paths.bridge.join("obd.json")).unwrap_or_default();
        let connected = json_bool(&raw, "connected");
        if connected {
            let dpf_regen = json_bool(&raw, "dpf_regen_active");
            let coolant = json_num(&raw, "coolant_c");
            let batt = json_num(&raw, "battery_v");
            let rpm = json_num(&raw, "rpm").unwrap_or(0.0);
            let dtcs: BTreeSet<String> = extract_dtc_codes(&raw).into_iter().collect();

            let mut candidate: Option<AlertFlash> = None;

            // 1. FAP Regen rising edge
            if dpf_regen && !am.was_dpf_regen {
                let t = json_num(&raw, "dpf_temp_c")
                    .map(|c| format!("Échappement {:.0} °C · Ne coupez pas le moteur !", c))
                    .unwrap_or_else(|| "Ne coupez pas le moteur jusqu'à la fin !".into());
                candidate = Some(AlertFlash {
                    active: true,
                    severity: "critical".into(),
                    title: "RÉGÉNÉRATION FAP EN COURS".into(),
                    subtitle: t,
                });
            }
            am.was_dpf_regen = dpf_regen;

            // 2. Engine Warm (80 °C) & Overheat (>= 102 °C)
            if let Some(c) = coolant {
                if c < 68.0 {
                    am.warm_armed = true;
                    if c < 55.0 { am.warm_announced = false; }
                } else if c >= 80.0 && am.warm_armed && !am.warm_announced && candidate.is_none() {
                    am.warm_armed = false;
                    am.warm_announced = true;
                    candidate = Some(AlertFlash {
                        active: true,
                        severity: "info".into(),
                        title: "MOTEUR À TEMPÉRATURE".into(),
                        subtitle: format!("Liquide de refroidissement à {:.0} °C · Moteur prêt", c),
                    });
                }
                if c >= 102.0 && am.last_overheat_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(90)) {
                    am.last_overheat_at = Some(now);
                    candidate = Some(AlertFlash {
                        active: true,
                        severity: "critical".into(),
                        title: "SURCHAUFFE MOTEUR !".into(),
                        subtitle: format!("Température eau critique : {:.0} °C · Ralentissez !", c),
                    });
                }
            }

            // 3. Low alternator voltage while running (< 13.4 V sustained 6 s)
            if let Some(v) = batt {
                if rpm >= 450.0 && v > 6.0 && v < 13.4 {
                    let since = *am.low_batt_since.get_or_insert(now);
                    if since.elapsed() >= Duration::from_secs(6)
                        && am.last_low_batt_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(300))
                        && candidate.is_none()
                    {
                        am.last_low_batt_at = Some(now);
                        candidate = Some(AlertFlash {
                            active: true,
                            severity: "warning".into(),
                            title: "CHARGE BATTERIE FAIBLE".into(),
                            subtitle: format!("Tension alternateur {:.2} V (< 13,4 V)", v),
                        });
                    }
                } else {
                    am.low_batt_since = None;
                }
            }

            // 4. New DTC detected
            if !am.dtc_ready {
                am.known_dtcs = dtcs;
                am.dtc_ready = true;
            } else {
                if let Some(new_code) = dtcs.difference(&am.known_dtcs).next().cloned() {
                    candidate = Some(AlertFlash {
                        active: true,
                        severity: "critical".into(),
                        title: format!("NOUVEAU DÉFAUT : {new_code}"),
                        subtitle: describe_dtc_fr(&new_code).into(),
                    });
                }
                am.known_dtcs = dtcs;
            }

            if let Some(flash) = candidate {
                am.active = Some((flash, now));
                ctrl_event = Some(9); // ALERT_FLASH_SHOW
            }
        }

        let current = am
            .active
            .as_ref()
            .map(|(f, _)| f.clone())
            .unwrap_or_default();
        (current, ctrl_event)
    }

    pub fn active_alert_flash(&self) -> AlertFlash {
        let am = self.alerts.borrow();
        am.active
            .as_ref()
            .map(|(f, _)| f.clone())
            .unwrap_or_default()
    }

    pub fn dismiss_alert_flash(&mut self) {
        let had = self.alerts.borrow_mut().active.take().is_some();
        if had {
            self.shell_action(10);
        }
    }

    pub fn trigger_test_flash(&mut self) {
        let flash = AlertFlash {
            active: true,
            severity: "critical".into(),
            title: "RÉGÉNÉRATION FAP EN COURS".into(),
            subtitle: "Échappement 624 °C · Ne coupez pas le moteur ! (Test Flash 4 s)".into(),
        };
        self.alerts.borrow_mut().active = Some((flash, Instant::now()));
        self.shell_action(9);
    }

    pub fn request_obd_post(&mut self, endpoint: &str, ok_msg: &str) {
        if self.dry_run {
            self.diag_feedback = format!("{ok_msg} (aperçu)");
            return;
        }
        let req = format!(
            "POST {endpoint} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        match TcpStream::connect_timeout(
            &"127.0.0.1:80".parse().unwrap(),
            Duration::from_millis(600),
        ) {
            Ok(mut stream) => {
                let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));
                if stream.write_all(req.as_bytes()).is_ok() {
                    self.diag_feedback = ok_msg.into();
                } else {
                    self.diag_feedback = "Erreur envoi requête OBD.".into();
                }
            }
            Err(_) => {
                self.diag_feedback = "Service OBD indisponible.".into();
            }
        }
    }

    pub fn oem_name(&self) -> String {
        Self::read_trimmed(&self.paths.settings.join("name.txt"))
            .filter(|s| valid_radio_name(s))
            .unwrap_or_else(|| "Autoradio".into())
    }

    pub fn radio_name(&self, kind: &str) -> String {
        let default = if kind == "wifi" {
            "CarFluxoid-Pi_AP"
        } else {
            "CarFluxoid"
        };
        Self::read_trimmed(&self.paths.settings.join(format!("{kind}-name.txt")))
            .unwrap_or_else(|| default.into())
    }

    fn read_options(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        if let Ok(content) = fs::read_to_string(self.paths.settings.join("options.txt")) {
            for line in content.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        map
    }

    pub fn option(&self, key: &str, fallback: &str) -> String {
        self.read_options()
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    pub fn set_option(&mut self, key: &str, value: &str) {
        self.cancel_pending_auto_start();
        let mut map = self.read_options();
        map.insert(key.to_string(), value.to_string());
        let mut out = String::new();
        for (k, v) in map {
            out.push_str(&k);
            out.push('=');
            out.push_str(&v);
            out.push('\n');
        }
        if let Err(err) = atomic_write(&self.paths.settings.join("options.txt"), out.as_bytes(), true)
        {
            self.notice = format!("Erreur sauvegarde : {err}");
        } else {
            self.notice = "Réglage enregistré · redémarrez CarPlay pour appliquer.".into();
        }
    }

    pub fn auto_start_enabled(&self) -> bool {
        matches!(self.option("auto_start", "true").as_str(), "true" | "1")
    }

    pub fn telemetry(&self) -> Telemetry {
        let version = option_env!("CARFLUXOID_VERSION")
            .map(|v| format!("v{v}"))
            .unwrap_or_else(|| "v1.0.2".into());
        let build = option_env!("CARFLUXOID_BUILD")
            .unwrap_or("local")
            .to_string();

        let cpu_temp = Self::read_trimmed(Path::new("/sys/class/thermal/thermal_zone0/temp"))
            .and_then(|s| s.parse::<f64>().ok())
            .map(|mc| format!("{:.1} °C", mc / 1000.0))
            .unwrap_or_else(|| {
                if self.dry_run {
                    "42.3 °C".into()
                } else {
                    "—".into()
                }
            });

        let (night_status, night_is_dark) =
            match Self::read_trimmed(&self.paths.bridge.join("video.sock.night")).as_deref() {
                Some("1") => ("Nuit".into(), true),
                Some("0") => ("Jour".into(), false),
                _ => {
                    if self.dry_run {
                        ("Nuit (auto)".into(), true)
                    } else {
                        ("Auto".into(), true)
                    }
                }
            };

        let gps_status = Self::read_trimmed(&self.paths.bridge.join("video.sock.gps_cp"))
            .or_else(|| {
                Self::read_trimmed(&self.paths.bridge.join("video.sock.gps"))
                    .map(|_| "Fix voiture".into())
            })
            .unwrap_or_else(|| {
                if self.dry_run {
                    "Fix actif".into()
                } else {
                    "En attente".into()
                }
            });

        let (w, h) = self.dimensions();
        Telemetry {
            version,
            build,
            cpu_temp,
            gps_status,
            night_status,
            night_is_dark,
            display_status: format!("{w} × {h}"),
        }
    }

    pub fn effective_day_mode(&self, tel: &Telemetry) -> bool {
        match self.option("theme", "auto").as_str() {
            "day" => true,
            "night" => false,
            _ => !tel.night_is_dark,
        }
    }

    pub fn cancel_pending_auto_start(&mut self) {
        if let Some((key, _)) = self.auto_start_pending.take() {
            self.auto_started_key = Some(key);
        }
    }

    pub fn shell_action(&mut self, value: u8) {
        if self.dry_run {
            self.notice = match value {
                5 => "CarPlay démarré (mode aperçu).".into(),
                6 => "Retour à l’autoradio (mode aperçu).".into(),
                7 => "Redémarrage CarPlay (mode aperçu).".into(),
                8 => "Retour au menu.".into(),
                _ => String::new(),
            };
            return;
        }
        let target = self.paths.bridge.join("video.sock.control");
        if let Some(sock) = &self.control_sock {
            if sock.send_to(&[value], &target).is_err() {
                self.notice = "Connexion à l’autoradio en cours…".into();
            }
        }
    }

    pub fn start_carplay(&mut self, restart: bool) {
        self.cancel_pending_auto_start();
        if self.dry_run {
            self.active_page = "home".into();
            self.shell_action(if restart { 7 } else { 5 });
            return;
        }
        let running = self
            .carplay_job
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .is_none()
            && self.carplay_job.is_some();

        if !running {
            let mut cmd = Command::new("/opt/carfluxoid/bin/start-ap-trial.sh");
            if restart {
                cmd.arg("--restart");
            }
            cmd.stdin(Stdio::null());
            match cmd.spawn() {
                Ok(child) => self.carplay_job = Some(child),
                Err(err) => {
                    eprintln!("DASHBOARD_CARPLAY_START_FAILED {err}");
                    self.notice = "Impossible de lancer le Wi-Fi CarFluxoid.".into();
                    return;
                }
            }
        }
        self.active_page = "home".into();
        self.shell_action(if restart { 7 } else { 5 });
    }

    pub fn check_carplay_job(&mut self) -> bool {
        let Some(child) = self.carplay_job.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.carplay_job = None;
                if status.success() {
                    return false;
                }
                eprintln!(
                    "DASHBOARD_CARPLAY_START_FAILED exit={:?}",
                    status.code()
                );
                self.shell_action(8);
                self.notice = "Le Wi-Fi ou le récepteur CarPlay n’a pas démarré.".into();
                true
            }
            Ok(None) => false,
            Err(_) => {
                self.carplay_job = None;
                false
            }
        }
    }

    fn display_session_key(&self) -> Option<(u64, i64, i64)> {
        let display_file = self.paths.bridge.join("video.sock.display");
        let control_file = self.paths.bridge.join("video.sock.control");
        let meta = fs::metadata(&display_file).ok()?;
        if !control_file.exists() {
            return None;
        }
        Some((meta.ino(), meta.mtime(), meta.mtime_nsec()))
    }

    pub fn maybe_auto_start(&mut self) -> bool {
        if self.dry_run || !self.auto_start_enabled() {
            return false;
        }
        let Some(key) = self.display_session_key() else {
            self.auto_start_pending = None;
            return false;
        };
        if self.auto_started_key == Some(key) {
            return false;
        }
        let now = Instant::now();
        match self.auto_start_pending {
            Some((pending_key, started)) if pending_key == key => {
                if now.duration_since(started) < Duration::from_secs(2) {
                    return false;
                }
            }
            _ => {
                self.auto_start_pending = Some((key, now));
                return false;
            }
        }
        self.auto_started_key = Some(key);
        self.auto_start_pending = None;
        println!("DASHBOARD_AUTO_START head_unit=connected");
        self.start_carplay(false);
        let (w, h) = self.dimensions();
        !self
            .paths
            .runtime
            .join(format!("dashboard-wait-{w}x{h}.h264"))
            .is_file()
    }

    pub fn open_editor(&mut self, kind: &str) {
        self.cancel_pending_auto_start();
        self.notice.clear();
        self.edit_target = kind.to_string();
        self.draft = self.radio_name(kind);
        self.shift = false;
        self.active_page = "keyboard".into();
    }

    pub fn save_editor(&mut self) {
        let name = self.draft.trim().to_string();
        if !valid_radio_name(&name) {
            self.notice = "Nom invalide · 1 à 32 caractères (A-Z, 0-9, . _ -)".into();
            return;
        }
        let file = self
            .paths
            .settings
            .join(format!("{}-name.txt", self.edit_target));
        if let Err(err) = atomic_write(&file, format!("{name}\n").as_bytes(), true) {
            self.notice = format!("Erreur d’enregistrement : {err}");
            return;
        }
        self.active_page = "network".into();
        self.notice = "Nom enregistré · actif au prochain démarrage.".into();
    }
}

pub fn valid_radio_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 32 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'))
}

pub fn atomic_write(path: &Path, data: &[u8], durable: bool) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.flush()?;
        if durable {
            f.sync_all()?;
        }
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_radio_names() {
        assert!(valid_radio_name("CarFluxoid-Pi_AP"));
        assert!(valid_radio_name("Fiat Tipo 356"));
        assert!(!valid_radio_name(""));
        assert!(!valid_radio_name(" leading"));
        assert!(!valid_radio_name("bad/name"));
        assert!(!valid_radio_name(&"a".repeat(33)));
    }
}
