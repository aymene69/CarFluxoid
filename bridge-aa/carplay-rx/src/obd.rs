//! OBD-II (ELM327 / Vgate iCar Pro BLE & Classic RFCOMM) telemetry, Diesel MultiJet
//! consumption estimator, French DTC decoder, and automatic GPS+OBD trip recorder.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

fn settings_dir() -> PathBuf {
    std::env::var_os("AA_SETTINGS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/persist/aa-settings".into())
}

fn diag_dir() -> PathBuf {
    std::env::var_os("AA_DIAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/carfluxoid".into())
}

fn bridge_dir() -> PathBuf {
    std::env::var_os("AA_BRIDGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/carfluxoid".into())
}

fn trips_dir() -> PathBuf {
    diag_dir().join("trips")
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpsFix {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt_m: Option<f64>,
    #[serde(default)]
    pub speed_kmh: Option<f64>,
    #[serde(default)]
    pub bearing: Option<f64>,
    #[serde(default)]
    pub ts: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DtcCode {
    pub code: String,
    pub pending: bool,
    pub description_fr: String,
    pub severity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BtDeviceInfo {
    pub mac: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub paired: bool,
    pub connected: bool,
    pub likely_obd: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TripSample {
    pub t: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<f64>,
    pub spd: f64,
    pub rpm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oil: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_lvl: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l100: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batt: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TripSummary {
    pub id: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub duration_s: u64,
    pub distance_km: f64,
    pub avg_speed_kmh: f64,
    pub max_speed_kmh: f64,
    pub max_rpm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_boost_bar: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_coolant_c: Option<f64>,
    pub fuel_used_l: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_l_per_100km: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_start_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_end_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_0_50_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_0_100_s: Option<f64>,
    pub eco_score: u8,
    pub sample_count: usize,
    pub has_gps: bool,
    pub has_obd: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TripRecord {
    pub summary: TripSummary,
    pub samples: Vec<TripSample>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObdLiveSnapshot {
    pub connected: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_name: Option<String>,
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elm_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub supported_pids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_kmh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oil_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intake_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_kpa: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baro_kpa: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boost_bar: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maf_gps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rail_bar: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_level_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_rate_lph: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_l_per_100km: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_v: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsFix>,
    pub dtcs: Vec<DtcCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_trip: Option<TripSummary>,
    pub updated_at: u64,
}

#[derive(Clone, Debug)]
enum ObdCommand {
    Connect(String),
    Disconnect,
    ReadDtcs,
    ClearDtcs,
}

struct ObdService {
    live: Mutex<ObdLiveSnapshot>,
    devices: Mutex<Vec<BtDeviceInfo>>,
    pending_cmd: Mutex<Option<ObdCommand>>,
}

static SERVICE: OnceLock<Arc<ObdService>> = OnceLock::new();

fn service() -> Arc<ObdService> {
    SERVICE
        .get_or_init(|| {
            let saved_mac = load_saved_mac();
            Arc::new(ObdService {
                live: Mutex::new(ObdLiveSnapshot {
                    state: if saved_mac.is_some() {
                        "connecting".into()
                    } else {
                        "disconnected".into()
                    },
                    adapter_mac: saved_mac,
                    transport: "none".into(),
                    ..Default::default()
                }),
                devices: Mutex::new(Vec::new()),
                pending_cmd: Mutex::new(None),
            })
        })
        .clone()
}

pub fn valid_mac(raw: &str) -> Option<String> {
    let mac = raw.trim();
    if mac.len() != 17 {
        return None;
    }
    for (i, b) in mac.bytes().enumerate() {
        if i % 3 == 2 {
            if b != b':' {
                return None;
            }
        } else if !b.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(mac.to_ascii_uppercase())
}

fn load_saved_mac() -> Option<String> {
    fs::read_to_string(settings_dir().join("obd-mac.txt"))
        .ok()
        .and_then(|s| valid_mac(&s))
}

fn save_mac(mac: Option<&str>) {
    let dir = settings_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("obd-mac.txt");
    match mac {
        Some(m) => {
            let _ = fs::write(path, format!("{m}\n"));
        }
        None => {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn is_likely_obd_name(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    [
        "V-LINK", "VLINK", "VGATE", "ICAR", "OBD", "ELM327", "VEEPEAK", "KONNWEI", "CARISTA",
    ]
    .iter()
    .any(|kw| up.contains(kw))
}

/// Parse an ELM327 hex response for a requested Mode 01 PID (e.g. pid=0x0C -> expects "41 0C ...").
pub fn parse_mode01_bytes(raw: &str, pid: u8) -> Option<Vec<u8>> {
    let mut hex = String::with_capacity(raw.len());
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("SEARCHING")
            || trimmed.starts_with("NO DATA")
            || trimmed.starts_with("STOPPED")
            || trimmed.starts_with("UNABLE")
            || trimmed.starts_with("ERROR")
            || trimmed.starts_with('?')
        {
            continue;
        }
        // Strip ISO-TP frame index prefix like "0:" / "1:" if present.
        let body = match trimmed.split_once(':') {
            Some((prefix, rest)) if prefix.len() <= 2 && prefix.chars().all(|c| c.is_ascii_hexdigit()) => {
                rest
            }
            _ => trimmed,
        };
        for c in body.chars() {
            if c.is_ascii_hexdigit() {
                hex.push(c.to_ascii_uppercase());
            }
        }
    }
    if hex.len() < 4 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == 0x41 && bytes[i + 1] == pid {
            return Some(bytes[i + 2..].to_vec());
        }
    }
    None
}

/// Decode supported PIDs from a 4-byte bitmap response to 0100/0120/0140/0160.
pub fn decode_supported_pids(base: u8, data: &[u8]) -> Vec<u8> {
    if data.len() < 4 {
        return Vec::new();
    }
    let mask = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let mut out = Vec::new();
    for bit in 0..32u32 {
        if (mask & (1u32 << (31 - bit))) != 0 {
            out.push(base + (bit as u8) + 1);
        }
    }
    out
}

/// Parse ELM327 `AT RV` battery voltage string (e.g. "14.2V" -> 14.2).
pub fn parse_at_rv(raw: &str) -> Option<f64> {
    for token in raw.split(|c: char| !c.is_ascii_digit() && c != '.') {
        if token.contains('.') {
            if let Ok(v) = token.parse::<f64>() {
                if (6.0..=18.5).contains(&v) {
                    return Some((v * 100.0).round() / 100.0);
                }
            }
        }
    }
    None
}

/// Decode ISO 15031-6 DTCs from Mode 03 (reply 0x43) or Mode 07 (reply 0x47).
pub fn parse_dtcs(raw: &str, mode_reply: u8, pending: bool) -> Vec<DtcCode> {
    let mut hex = String::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("NO DATA") || t.starts_with("SEARCHING") {
            continue;
        }
        let body = match t.split_once(':') {
            Some((prefix, rest)) if prefix.len() <= 2 && prefix.chars().all(|c| c.is_ascii_hexdigit()) => {
                rest
            }
            _ => t,
        };
        for c in body.chars() {
            if c.is_ascii_hexdigit() {
                hex.push(c.to_ascii_uppercase());
            }
        }
    }
    if hex.len() < 4 || !hex.len().is_multiple_of(2) {
        return Vec::new();
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    let Some(pos) = bytes.iter().position(|&b| b == mode_reply) else {
        return Vec::new();
    };
    let mut payload = &bytes[pos + 1..];
    // CAN ISO 15765-4 includes a DTC count byte right after 0x43/0x47 when payload length is odd.
    if payload.len() % 2 == 1 {
        payload = &payload[1..];
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for pair in payload.chunks_exact(2) {
        if pair[0] == 0 && pair[1] == 0 {
            continue;
        }
        let prefix = match (pair[0] >> 6) & 0x03 {
            0 => 'P',
            1 => 'C',
            2 => 'B',
            _ => 'U',
        };
        let d1 = (pair[0] >> 4) & 0x03;
        let d2 = pair[0] & 0x0F;
        let d3 = (pair[1] >> 4) & 0x0F;
        let d4 = pair[1] & 0x0F;
        let code = format!("{prefix}{d1}{d2:X}{d3:X}{d4:X}");
        if seen.insert(code.clone()) {
            let (desc, sev) = describe_dtc_fr(&code);
            out.push(DtcCode {
                code,
                pending,
                description_fr: desc.to_string(),
                severity: sev.to_string(),
            });
        }
    }
    out
}

pub fn describe_dtc_fr(code: &str) -> (&'static str, &'static str) {
    match code {
        "P0087" => ("Pression rampe carburant (rail) trop basse", "critical"),
        "P0088" => ("Pression rampe carburant (rail) trop élevée", "critical"),
        "P0089" => ("Régulateur de pression carburant — performance", "warning"),
        "P0090" | "P0091" | "P0092" => ("Circuit électrovanne régulateur pression carburant", "warning"),
        "P0093" => ("Fuite importante circuit carburant haute pression", "critical"),
        "P0100" | "P0101" | "P0102" | "P0103" => ("Débitmètre d'air massique (MAF) — signal hors plage", "warning"),
        "P0105" | "P0106" | "P0107" | "P0108" => ("Capteur pression admission / turbo (MAP)", "warning"),
        "P0110" | "P0111" | "P0112" | "P0113" => ("Capteur température air d'admission (IAT)", "info"),
        "P0115" | "P0116" | "P0117" | "P0118" => ("Capteur température liquide de refroidissement", "warning"),
        "P0128" => ("Thermostat — température moteur trop basse", "warning"),
        "P0190" | "P0191" | "P0192" | "P0193" => ("Capteur pression rampe commune (rail diesel)", "critical"),
        "P0201" => ("Injecteur cylindre 1 — défaut circuit", "critical"),
        "P0202" => ("Injecteur cylindre 2 — défaut circuit", "critical"),
        "P0203" => ("Injecteur cylindre 3 — défaut circuit", "critical"),
        "P0204" => ("Injecteur cylindre 4 — défaut circuit", "critical"),
        "P0234" => ("Suralimentation turbo — pression excessive (overboost)", "critical"),
        "P0235" | "P0236" | "P0237" | "P0238" => ("Capteur pression turbo (Boost A)", "warning"),
        "P0299" => ("Suralimentation turbo — pression insuffisante (underboost)", "warning"),
        "P0300" => ("Ratés d'allumage / combustion détectés (aléatoires)", "critical"),
        "P0301" | "P0302" | "P0303" | "P0304" => ("Ratés de combustion sur un cylindre", "critical"),
        "P0335" | "P0336" => ("Capteur position vilebrequin (PMH)", "critical"),
        "P0340" | "P0341" => ("Capteur position arbre à cames", "critical"),
        "P0380" | "P0381" => ("Circuit bougies de préchauffage diesel", "warning"),
        "P0400" | "P0401" => ("Vanne EGR — débit insuffisant (encrassement)", "warning"),
        "P0402" => ("Vanne EGR — débit excessif (bloquée ouverte)", "warning"),
        "P0403" | "P0404" | "P0405" | "P0406" | "P0409" => ("Vanne EGR — défaut commande / position", "warning"),
        "P0420" => ("Efficacité catalyseur sous le seuil", "warning"),
        "P0480" | "P0481" => ("Circuit ventilateur de refroidissement moteur", "warning"),
        "P0500" => ("Capteur vitesse véhicule (VSS)", "warning"),
        "P0560" | "P0562" | "P0563" => ("Tension d'alimentation système / batterie anormale", "warning"),
        "P0670" | "P0671" | "P0672" | "P0673" | "P0674" => ("Bougie de préchauffage défectueuse (cylindre 1–4)", "info"),
        "P0683" => ("Communication module de préchauffage diesel", "warning"),
        "P1206" => ("Filtre à particules (FAP/DPF) Fiat — colmatage / régénération", "warning"),
        "P2002" => ("Filtre à particules (FAP/DPF) — efficacité sous le seuil", "warning"),
        "P2009" | "P2015" => ("Volets turbulence admission (Swirl) — position", "warning"),
        "P2080" | "P2084" => ("Sonde température gaz d'échappement (EGT / FAP)", "warning"),
        "P2263" => ("Performance système de suralimentation turbo", "warning"),
        "P242F" => ("Filtre à particules (FAP/DPF) — accumulation de cendres", "warning"),
        "P2452" | "P2453" | "P2454" | "P2455" => ("Capteur pression différentielle FAP/DPF", "warning"),
        "P2458" | "P2459" => ("Durée ou fréquence régénération FAP/DPF anormale", "warning"),
        "P2463" => ("Filtre à particules (FAP/DPF) — accumulation de suie critique", "critical"),
        "U0001" | "U0100" | "U0121" | "U0140" => ("Perte communication bus CAN avec un calculateur", "warning"),
        _ => match code.chars().next() {
            Some('P') => ("Défaut groupe motopropulseur (OBD2)", "warning"),
            Some('C') => ("Défaut châssis (ABS / ESP / direction)", "warning"),
            Some('B') => ("Défaut carrosserie / habitacle (BCM)", "info"),
            Some('U') => ("Défaut réseau communication CAN", "warning"),
            _ => ("Code défaut OBD2 inconnu", "info"),
        },
    }
}

/// Estimate Diesel MultiJet fuel rate (L/h) from direct PID 0x5E, or MAF + Load, or MAP + RPM + IAT.
pub fn estimate_diesel_fuel_lph(
    direct_fuel_rate_lph: Option<f64>,
    maf_gps: Option<f64>,
    load_pct: Option<f64>,
    map_kpa: Option<f64>,
    rpm: Option<f64>,
    intake_c: Option<f64>,
) -> Option<f64> {
    if let Some(rate) = direct_fuel_rate_lph {
        if (0.0..=60.0).contains(&rate) {
            return Some((rate * 100.0).round() / 100.0);
        }
    }
    const DIESEL_DENSITY_KG_L: f64 = 0.835;
    let load = load_pct.unwrap_or(25.0).clamp(0.0, 100.0);
    // Diesel MultiJet runs lean at idle (~52:1) down to ~18:1 under full boost/load.
    let afr = (52.0 - 0.34 * load).clamp(18.0, 55.0);
    let maf = maf_gps.or_else(|| {
        // Speed-density fallback for 1.6L MultiJet if MAF isn't exposed.
        let (map, r) = (map_kpa?, rpm?);
        if r < 300.0 {
            return Some(0.0);
        }
        let temp_k = (intake_c.unwrap_or(25.0) + 273.15).clamp(230.0, 380.0);
        let disp_l = 1.598;
        let ve = 0.88;
        // Ideal gas law: air_g_s = (MAP_kPa * V_L / (R * T_K)) * (RPM / 120) * M_air * VE
        let moles_per_s = (map * disp_l / (8.3145 * temp_k)) * (r / 120.0) * ve;
        Some((moles_per_s * 28.97).max(0.0))
    })?;
    let fuel_g_s = maf / afr;
    let lph = (fuel_g_s * 3.6) / DIESEL_DENSITY_KG_L;
    Some((lph.clamp(0.0, 60.0) * 100.0).round() / 100.0)
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().atan2((1.0 - a).sqrt())
}

fn read_car_gps() -> Option<GpsFix> {
    let text = fs::read_to_string(bridge_dir().join("video.sock.gps.json")).ok()?;
    let fix: GpsFix = serde_json::from_str(&text).ok()?;
    let now = now_unix_secs();
    if fix.ts > 0 && now > fix.ts && now - fix.ts > 10 {
        return None;
    }
    if fix.lat == 0.0 && fix.lon == 0.0 {
        return None;
    }
    Some(fix)
}

fn write_dashboard_obd_snapshot(live: &ObdLiveSnapshot) {
    let dir = bridge_dir();
    if !dir.is_dir() {
        return;
    }
    let tmp = dir.join("obd.json.tmp");
    let dst = dir.join("obd.json");
    if let Ok(json) = serde_json::to_string(live) {
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &dst);
        }
    }
}

struct ActiveTrip {
    id: String,
    started_at: u64,
    last_active_at: Instant,
    last_sample_at: Instant,
    last_checkpoint_at: Instant,
    samples: Vec<TripSample>,
    distance_km: f64,
    fuel_used_l: f64,
    max_speed_kmh: f64,
    max_rpm: f64,
    max_boost_bar: Option<f64>,
    max_coolant_c: Option<f64>,
    fuel_start_pct: Option<f64>,
    fuel_end_pct: Option<f64>,
    accel_armed: bool,
    accel_start: Option<Instant>,
    best_0_50_s: Option<f64>,
    best_0_100_s: Option<f64>,
    harsh_events: u32,
    last_speed_kmh: f64,
    last_gps: Option<(f64, f64)>,
    has_gps: bool,
    has_obd: bool,
}

impl ActiveTrip {
    fn start(now: u64, live: &ObdLiveSnapshot) -> Self {
        let now_i = Instant::now();
        Self {
            id: format!("trip-{now}"),
            started_at: now,
            last_active_at: now_i,
            last_sample_at: now_i - Duration::from_secs(2),
            last_checkpoint_at: now_i,
            samples: Vec::with_capacity(512),
            distance_km: 0.0,
            fuel_used_l: 0.0,
            max_speed_kmh: 0.0,
            max_rpm: 0.0,
            max_boost_bar: None,
            max_coolant_c: None,
            fuel_start_pct: live.fuel_level_pct,
            fuel_end_pct: live.fuel_level_pct,
            accel_armed: true,
            accel_start: None,
            best_0_50_s: None,
            best_0_100_s: None,
            harsh_events: 0,
            last_speed_kmh: 0.0,
            last_gps: live.gps.as_ref().map(|g| (g.lat, g.lon)),
            has_gps: live.gps.is_some(),
            has_obd: live.connected,
        }
    }

    fn ingest(&mut self, live: &ObdLiveSnapshot) {
        let now_i = Instant::now();
        let dt_s = now_i
            .saturating_duration_since(self.last_sample_at)
            .as_secs_f64()
            .clamp(0.1, 5.0);
        let spd = live
            .speed_kmh
            .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
            .unwrap_or(0.0)
            .max(0.0);
        let rpm = live.rpm.unwrap_or(0.0).max(0.0);

        // 0-50 & 0-100 km/h automatic launch detector.
        if spd <= 2.0 {
            self.accel_armed = true;
            self.accel_start = None;
        } else if self.accel_armed && self.accel_start.is_none() && spd > 3.0 {
            self.accel_start = Some(now_i);
        } else if let Some(t0) = self.accel_start {
            let elapsed = now_i.saturating_duration_since(t0).as_secs_f64();
            if elapsed > 25.0 {
                self.accel_start = None;
                self.accel_armed = false;
            } else {
                if spd >= 50.0 && self.best_0_50_s.is_none_or(|b| elapsed < b) && elapsed >= 1.5 {
                    self.best_0_50_s = Some((elapsed * 100.0).round() / 100.0);
                }
                if spd >= 100.0 {
                    if self.best_0_100_s.is_none_or(|b| elapsed < b) && elapsed >= 4.0 {
                        self.best_0_100_s = Some((elapsed * 100.0).round() / 100.0);
                    }
                    self.accel_start = None;
                    self.accel_armed = false;
                }
            }
        }

        if dt_s < 0.95 {
            return;
        }
        self.last_sample_at = now_i;

        if spd >= 2.0 || rpm >= 450.0 {
            self.last_active_at = now_i;
        }
        if live.connected {
            self.has_obd = true;
        }
        // Integrate distance from OBD speed (or GPS Haversine fallback).
        if live.speed_kmh.is_some() {
            self.distance_km += spd * (dt_s / 3600.0);
        } else if let Some(g) = &live.gps {
            if let Some((plat, plon)) = self.last_gps {
                let d = haversine_km(plat, plon, g.lat, g.lon);
                if d < 0.5 {
                    self.distance_km += d;
                }
            }
            self.last_gps = Some((g.lat, g.lon));
        }
        if let Some(g) = &live.gps {
            self.has_gps = true;
            self.last_gps = Some((g.lat, g.lon));
        }
        if let Some(lph) = live.fuel_rate_lph {
            self.fuel_used_l += lph * (dt_s / 3600.0);
        }
        if spd > self.max_speed_kmh {
            self.max_speed_kmh = spd;
        }
        if rpm > self.max_rpm {
            self.max_rpm = rpm;
        }
        if let Some(b) = live.boost_bar {
            self.max_boost_bar = Some(self.max_boost_bar.map_or(b, |m| m.max(b)));
        }
        if let Some(c) = live.coolant_c {
            self.max_coolant_c = Some(self.max_coolant_c.map_or(c, |m| m.max(c)));
        }
        if let Some(fl) = live.fuel_level_pct {
            if self.fuel_start_pct.is_none() {
                self.fuel_start_pct = Some(fl);
            }
            self.fuel_end_pct = Some(fl);
        }
        // Track harsh acceleration/braking or high-RPM driving for eco score.
        let accel_kmh_s = (spd - self.last_speed_kmh).abs() / dt_s;
        if accel_kmh_s > 11.0 || rpm > 3600.0 {
            self.harsh_events = self.harsh_events.saturating_add(1);
        }
        self.last_speed_kmh = spd;

        let elapsed_s = now_unix_secs().saturating_sub(self.started_at);
        if self.samples.len() < 14_400 {
            self.samples.push(TripSample {
                t: elapsed_s,
                lat: live.gps.as_ref().map(|g| (g.lat * 1e6).round() / 1e6),
                lon: live.gps.as_ref().map(|g| (g.lon * 1e6).round() / 1e6),
                alt: live.gps.as_ref().and_then(|g| g.alt_m),
                spd: (spd * 10.0).round() / 10.0,
                rpm: rpm.round(),
                boost: live.boost_bar,
                load: live.load_pct,
                coolant: live.coolant_c,
                oil: live.oil_c,
                fuel_lvl: live.fuel_level_pct,
                l100: live.fuel_l_per_100km,
                batt: live.battery_v,
            });
        }
    }

    fn summary(&self) -> TripSummary {
        let ended_at = now_unix_secs().max(self.started_at);
        let duration_s = ended_at.saturating_sub(self.started_at).max(1);
        let avg_speed = if duration_s > 0 {
            self.distance_km / (duration_s as f64 / 3600.0)
        } else {
            0.0
        };
        let avg_l100 = if self.distance_km >= 0.05 && self.fuel_used_l > 0.0 {
            Some(((self.fuel_used_l / self.distance_km) * 100.0 * 10.0).round() / 10.0)
        } else {
            None
        };
        let penalty = (self.harsh_events as f64 * 2.5).min(60.0);
        let eco_score = (100.0 - penalty).clamp(20.0, 100.0).round() as u8;
        TripSummary {
            id: self.id.clone(),
            started_at: self.started_at,
            ended_at,
            duration_s,
            distance_km: (self.distance_km * 100.0).round() / 100.0,
            avg_speed_kmh: (avg_speed * 10.0).round() / 10.0,
            max_speed_kmh: (self.max_speed_kmh * 10.0).round() / 10.0,
            max_rpm: self.max_rpm.round(),
            max_boost_bar: self.max_boost_bar.map(|v| (v * 100.0).round() / 100.0),
            max_coolant_c: self.max_coolant_c,
            fuel_used_l: (self.fuel_used_l * 1000.0).round() / 1000.0,
            avg_l_per_100km: avg_l100,
            fuel_start_pct: self.fuel_start_pct,
            fuel_end_pct: self.fuel_end_pct,
            best_0_50_s: self.best_0_50_s,
            best_0_100_s: self.best_0_100_s,
            eco_score,
            sample_count: self.samples.len(),
            has_gps: self.has_gps,
            has_obd: self.has_obd,
        }
    }

    fn save(&self) {
        if self.samples.len() < 5 && self.distance_km < 0.02 {
            return;
        }
        let dir = trips_dir();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let record = TripRecord {
            summary: self.summary(),
            samples: self.samples.clone(),
        };
        let tmp = dir.join(format!("{}.json.tmp", self.id));
        let dst = dir.join(format!("{}.json", self.id));
        if let Ok(json) = serde_json::to_string(&record) {
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, &dst);
            }
        }
        prune_old_trips(&dir, 200);
    }
}

fn prune_old_trips(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for old in &files[..files.len() - keep] {
        let _ = fs::remove_file(old);
    }
}

pub fn list_trips_json() -> String {
    let dir = trips_dir();
    let mut summaries = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&p) {
                if let Ok(rec) = serde_json::from_str::<TripRecord>(&text) {
                    summaries.push(rec.summary);
                }
            }
        }
    }
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".into())
}

fn valid_trip_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn get_trip_json(id: &str) -> Option<String> {
    if !valid_trip_id(id) {
        return None;
    }
    fs::read_to_string(trips_dir().join(format!("{id}.json"))).ok()
}

pub fn delete_trip(id: &str) -> bool {
    if !valid_trip_id(id) {
        return false;
    }
    fs::remove_file(trips_dir().join(format!("{id}.json"))).is_ok()
}

pub fn trip_to_gpx(id: &str) -> Option<String> {
    let text = get_trip_json(id)?;
    let rec: TripRecord = serde_json::from_str(&text).ok()?;
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <gpx version=\"1.1\" creator=\"CarFluxoid\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n\
         <trk><name>",
    );
    out.push_str(&rec.summary.id);
    out.push_str("</name><trkseg>\n");
    for s in &rec.samples {
        if let (Some(lat), Some(lon)) = (s.lat, s.lon) {
            out.push_str(&format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\">"));
            if let Some(alt) = s.alt {
                out.push_str(&format!("<ele>{alt:.1}</ele>"));
            }
            out.push_str("</trkpt>\n");
        }
    }
    out.push_str("</trkseg></trk></gpx>\n");
    Some(out)
}

pub fn live_json() -> String {
    let svc = service();
    let live = svc.live.lock().unwrap().clone();
    serde_json::to_string(&live).unwrap_or_else(|_| "{}".into())
}

pub fn devices_json() -> String {
    let svc = service();
    let devs = svc.devices.lock().unwrap().clone();
    serde_json::to_string(&devs).unwrap_or_else(|_| "[]".into())
}

pub fn request_connect(mac: &str) -> Result<(), String> {
    let norm = valid_mac(mac).ok_or_else(|| "Adresse MAC Bluetooth invalide.".to_string())?;
    save_mac(Some(&norm));
    let svc = service();
    {
        let mut live = svc.live.lock().unwrap();
        live.adapter_mac = Some(norm.clone());
        live.state = "connecting".into();
        live.error = None;
    }
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::Connect(norm));
    Ok(())
}

pub fn request_disconnect() {
    save_mac(None);
    let svc = service();
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::Disconnect);
    let mut live = svc.live.lock().unwrap();
    live.connected = false;
    live.state = "disconnected".into();
    live.adapter_mac = None;
    live.adapter_name = None;
    live.transport = "none".into();
    live.error = None;
}

pub fn request_read_dtcs() {
    let svc = service();
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::ReadDtcs);
}

pub fn request_clear_dtcs() {
    let svc = service();
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::ClearDtcs);
}

pub fn scan_devices_sync() -> Result<String, String> {
    let svc = service();
    #[cfg(target_os = "linux")]
    {
        let list = bt_linux::scan_devices(Duration::from_secs(6))?;
        *svc.devices.lock().unwrap() = list.clone();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = svc;
        Ok("[]".into())
    }
}

pub fn start_background() {
    let svc = service();
    std::thread::Builder::new()
        .name("carfluxoid-obd".into())
        .spawn(move || obd_worker_loop(svc))
        .ok();
}

trait ElmTransport {
    fn transport_name(&self) -> &'static str;
    fn send_command(&mut self, cmd: &str, timeout: Duration) -> Result<String, String>;
}

fn obd_worker_loop(svc: Arc<ObdService>) {
    let mut active_trip: Option<ActiveTrip> = None;
    let mut last_reconnect = Instant::now() - Duration::from_secs(60);

    loop {
        // Always update GPS & trip logger even when no OBD adapter is paired/connected.
        let gps = read_car_gps();
        {
            let mut live = svc.live.lock().unwrap();
            live.gps = gps.clone();
            live.updated_at = now_unix_secs();
            update_trip_state(&mut active_trip, &mut live);
            write_dashboard_obd_snapshot(&live);
        }

        // Check pending disconnect/connect command.
        if let Some(cmd) = svc.pending_cmd.lock().unwrap().take() {
            match cmd {
                ObdCommand::Disconnect => {
                    continue;
                }
                ObdCommand::Connect(mac) => {
                    last_reconnect = Instant::now() - Duration::from_secs(60);
                    svc.live.lock().unwrap().adapter_mac = Some(mac);
                }
                ObdCommand::ReadDtcs | ObdCommand::ClearDtcs => {}
            }
        }

        let target_mac = svc.live.lock().unwrap().adapter_mac.clone();
        let Some(mac) = target_mac else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };

        if last_reconnect.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        last_reconnect = Instant::now();

        #[cfg(target_os = "linux")]
        {
            {
                let mut live = svc.live.lock().unwrap();
                live.state = "connecting".into();
                live.error = None;
                write_dashboard_obd_snapshot(&live);
            }
            match bt_linux::connect_elm(&mac) {
                Ok((mut transport, dev_name)) => {
                    eprintln!("OBD_CONNECTED mac={mac} name={dev_name:?} transport={}", transport.transport_name());
                    {
                        let mut live = svc.live.lock().unwrap();
                        live.state = "initializing".into();
                        live.adapter_name = Some(dev_name);
                        live.transport = transport.transport_name().into();
                    }
                    run_elm_session(&svc, transport.as_mut(), &mut active_trip);
                    {
                        let mut live = svc.live.lock().unwrap();
                        live.connected = false;
                        if live.adapter_mac.is_some() {
                            live.state = "reconnecting".into();
                        } else {
                            live.state = "disconnected".into();
                        }
                        write_dashboard_obd_snapshot(&live);
                    }
                }
                Err(err) => {
                    let mut live = svc.live.lock().unwrap();
                    live.connected = false;
                    live.state = "waiting".into();
                    live.error = Some(err);
                    write_dashboard_obd_snapshot(&live);
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = mac;
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn update_trip_state(active_trip: &mut Option<ActiveTrip>, live: &mut ObdLiveSnapshot) {
    let spd = live
        .speed_kmh
        .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
        .unwrap_or(0.0);
    let rpm = live.rpm.unwrap_or(0.0);
    let moving_or_running = spd >= 2.0 || rpm >= 450.0;

    if active_trip.is_none() && moving_or_running {
        let trip = ActiveTrip::start(now_unix_secs(), live);
        eprintln!("OBD_TRIP_START id={}", trip.id);
        *active_trip = Some(trip);
    }

    if let Some(trip) = active_trip.as_mut() {
        trip.ingest(live);
        if trip.last_checkpoint_at.elapsed() >= Duration::from_secs(30) {
            trip.save();
            trip.last_checkpoint_at = Instant::now();
        }
        let summary = trip.summary();
        live.active_trip = Some(summary);
        if trip.last_active_at.elapsed() >= Duration::from_secs(90) {
            eprintln!("OBD_TRIP_END id={} km={:.2}", trip.id, trip.distance_km);
            trip.save();
            *active_trip = None;
            live.active_trip = None;
        }
    }
}

#[cfg(target_os = "linux")]
fn run_elm_session(
    svc: &Arc<ObdService>,
    transport: &mut dyn ElmTransport,
    active_trip: &mut Option<ActiveTrip>,
) {
    let init_cmds = ["ATZ", "ATE0", "ATL0", "ATS0", "ATH0", "ATAT1", "ATSP0"];
    let mut elm_ver = None;
    for cmd in init_cmds {
        let timeout = if cmd == "ATZ" {
            Duration::from_millis(2500)
        } else {
            Duration::from_millis(1200)
        };
        match transport.send_command(cmd, timeout) {
            Ok(resp) => {
                if cmd == "ATZ" {
                    let v = resp
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty() && !l.starts_with("ATZ"))
                        .map(str::to_string);
                    elm_ver = v;
                }
            }
            Err(e) => {
                eprintln!("OBD_INIT_ERR cmd={cmd}: {e}");
                svc.live.lock().unwrap().error = Some(format!("Init {cmd}: {e}"));
                return;
            }
        }
    }

    // Discover supported Mode 01 PIDs (0100, 0120, 0140, 0160).
    let mut supported: BTreeSet<u8> = BTreeSet::new();
    for base in [0x00u8, 0x20, 0x40, 0x60] {
        let cmd = format!("01{:02X}", base);
        let Ok(resp) = transport.send_command(&cmd, Duration::from_millis(2500)) else {
            break;
        };
        let Some(data) = parse_mode01_bytes(&resp, base) else {
            break;
        };
        let pids = decode_supported_pids(base, &data);
        let has_next = pids.contains(&(base + 0x20));
        supported.extend(pids);
        if !has_next {
            break;
        }
    }
    let protocol = transport
        .send_command("ATDP", Duration::from_millis(1000))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let supported_labels: Vec<String> = supported.iter().map(|p| format!("01{p:02X}")).collect();
    eprintln!(
        "OBD_READY elm={elm_ver:?} proto={protocol:?} pids={}",
        supported_labels.len()
    );

    // Initial DTC read.
    let mut dtcs = read_all_dtcs(transport);
    {
        let mut live = svc.live.lock().unwrap();
        live.connected = true;
        live.state = "connected".into();
        live.error = None;
        live.elm_version = elm_ver;
        live.protocol = protocol;
        live.supported_pids = supported_labels;
        live.dtcs = dtcs.clone();
    }

    let has = |p: u8| supported.is_empty() || supported.contains(&p);
    let mut last_slow = Instant::now() - Duration::from_secs(30);
    let mut consec_errors = 0u32;

    loop {
        if let Some(cmd) = svc.pending_cmd.lock().unwrap().take() {
            match cmd {
                ObdCommand::Disconnect => return,
                ObdCommand::Connect(new_mac) => {
                    svc.live.lock().unwrap().adapter_mac = Some(new_mac);
                    return;
                }
                ObdCommand::ReadDtcs => {
                    dtcs = read_all_dtcs(transport);
                    svc.live.lock().unwrap().dtcs = dtcs.clone();
                }
                ObdCommand::ClearDtcs => {
                    let _ = transport.send_command("04", Duration::from_millis(2000));
                    std::thread::sleep(Duration::from_millis(400));
                    dtcs = read_all_dtcs(transport);
                    svc.live.lock().unwrap().dtcs = dtcs.clone();
                }
            }
        }

        let mut ok_this_cycle = false;
        // Fast tier: RPM, Speed, Load, MAP (Boost), Throttle, MAF, Direct Fuel Rate.
        let mut rpm = None;
        let mut speed = None;
        let mut load = None;
        let mut map = None;
        let mut throttle = None;
        let mut maf = None;
        let mut direct_fuel_lph = None;

        if has(0x0C) {
            if let Ok(r) = transport.send_command("010C", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x0C) {
                    if d.len() >= 2 {
                        rpm = Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 4.0);
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if has(0x0D) {
            if let Ok(r) = transport.send_command("010D", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x0D) {
                    if !d.is_empty() {
                        speed = Some(f64::from(d[0]));
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if has(0x04) {
            if let Ok(r) = transport.send_command("0104", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x04) {
                    if !d.is_empty() {
                        load = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if has(0x0B) {
            if let Ok(r) = transport.send_command("010B", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x0B) {
                    if !d.is_empty() {
                        map = Some(f64::from(d[0]));
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if has(0x11) {
            if let Ok(r) = transport.send_command("0111", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x11) {
                    if !d.is_empty() {
                        throttle = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if supported.contains(&0x10) {
            if let Ok(r) = transport.send_command("0110", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x10) {
                    if d.len() >= 2 {
                        maf = Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 100.0);
                        ok_this_cycle = true;
                    }
                }
            }
        }
        if supported.contains(&0x5E) {
            if let Ok(r) = transport.send_command("015E", Duration::from_millis(700)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x5E) {
                    if d.len() >= 2 {
                        direct_fuel_lph =
                            Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 20.0);
                        ok_this_cycle = true;
                    }
                }
            }
        }

        // Slow tier every 5 seconds: Coolant, Oil, Intake, Baro, Rail, Fuel Level, Ambient, Voltage.
        let do_slow = last_slow.elapsed() >= Duration::from_secs(5);
        let mut coolant = None;
        let mut oil = None;
        let mut intake = None;
        let mut baro = None;
        let mut rail_bar = None;
        let mut fuel_lvl = None;
        let mut ambient = None;
        let mut batt_v = None;

        if do_slow {
            last_slow = Instant::now();
            if has(0x05) {
                if let Ok(r) = transport.send_command("0105", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x05) {
                        if !d.is_empty() {
                            coolant = Some(f64::from(d[0]) - 40.0);
                            ok_this_cycle = true;
                        }
                    }
                }
            }
            if has(0x0F) {
                if let Ok(r) = transport.send_command("010F", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x0F) {
                        if !d.is_empty() {
                            intake = Some(f64::from(d[0]) - 40.0);
                        }
                    }
                }
            }
            if supported.contains(&0x5C) {
                if let Ok(r) = transport.send_command("015C", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x5C) {
                        if !d.is_empty() {
                            oil = Some(f64::from(d[0]) - 40.0);
                        }
                    }
                }
            }
            if supported.contains(&0x33) {
                if let Ok(r) = transport.send_command("0133", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x33) {
                        if !d.is_empty() {
                            baro = Some(f64::from(d[0]));
                        }
                    }
                }
            }
            if supported.contains(&0x23) {
                if let Ok(r) = transport.send_command("0123", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x23) {
                        if d.len() >= 2 {
                            // kPa = (A*256+B)*10 -> bar = kPa / 100 = raw / 10.
                            let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                            rail_bar = Some((f64::from(raw) / 10.0).round());
                        }
                    }
                }
            }
            if supported.contains(&0x2F) {
                if let Ok(r) = transport.send_command("012F", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x2F) {
                        if !d.is_empty() {
                            fuel_lvl = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                        }
                    }
                }
            }
            if supported.contains(&0x46) {
                if let Ok(r) = transport.send_command("0146", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x46) {
                        if !d.is_empty() {
                            ambient = Some(f64::from(d[0]) - 40.0);
                        }
                    }
                }
            }
            if supported.contains(&0x42) {
                if let Ok(r) = transport.send_command("0142", Duration::from_millis(800)) {
                    if let Some(d) = parse_mode01_bytes(&r, 0x42) {
                        if d.len() >= 2 {
                            let mv = u16::from(d[0]) * 256 + u16::from(d[1]);
                            batt_v = Some((f64::from(mv) / 10.0).round() / 100.0);
                        }
                    }
                }
            }
            if batt_v.is_none() {
                if let Ok(r) = transport.send_command("AT RV", Duration::from_millis(800)) {
                    batt_v = parse_at_rv(&r);
                    if batt_v.is_some() {
                        ok_this_cycle = true;
                    }
                }
            }
        }

        if ok_this_cycle {
            consec_errors = 0;
        } else {
            consec_errors += 1;
            if consec_errors >= 12 {
                eprintln!("OBD_LOST_LINK after {consec_errors} empty cycles");
                return;
            }
        }

        let gps = read_car_gps();
        {
            let mut live = svc.live.lock().unwrap();
            live.gps = gps;
            if rpm.is_some() {
                live.rpm = rpm;
            }
            if speed.is_some() {
                live.speed_kmh = speed;
            }
            if load.is_some() {
                live.load_pct = load;
            }
            if map.is_some() {
                live.map_kpa = map;
            }
            if throttle.is_some() {
                live.throttle_pct = throttle;
            }
            if maf.is_some() {
                live.maf_gps = maf;
            }
            if coolant.is_some() {
                live.coolant_c = coolant;
            }
            if oil.is_some() {
                live.oil_c = oil;
            }
            if intake.is_some() {
                live.intake_c = intake;
            }
            if baro.is_some() {
                live.baro_kpa = baro;
            }
            if rail_bar.is_some() {
                live.rail_bar = rail_bar;
            }
            if fuel_lvl.is_some() {
                live.fuel_level_pct = fuel_lvl;
            }
            if ambient.is_some() {
                live.ambient_c = ambient;
            }
            if batt_v.is_some() {
                live.battery_v = batt_v;
            }
            if let Some(m) = live.map_kpa {
                let b = live.baro_kpa.unwrap_or(101.3);
                live.boost_bar = Some(((m - b) / 100.0 * 100.0).round() / 100.0);
            }
            let lph = estimate_diesel_fuel_lph(
                direct_fuel_lph,
                live.maf_gps,
                live.load_pct,
                live.map_kpa,
                live.rpm,
                live.intake_c,
            );
            live.fuel_rate_lph = lph;
            let spd_for_cons = live
                .speed_kmh
                .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
                .unwrap_or(0.0);
            live.fuel_l_per_100km = if spd_for_cons >= 5.0 {
                lph.map(|r| ((r / spd_for_cons * 100.0).min(45.0) * 10.0).round() / 10.0)
            } else {
                None
            };
            live.updated_at = now_unix_secs();
            update_trip_state(active_trip, &mut live);
            write_dashboard_obd_snapshot(&live);
        }

        std::thread::sleep(Duration::from_millis(120));
    }
}

#[cfg(target_os = "linux")]
fn read_all_dtcs(transport: &mut dyn ElmTransport) -> Vec<DtcCode> {
    let mut all = Vec::new();
    if let Ok(r) = transport.send_command("03", Duration::from_millis(2000)) {
        all.extend(parse_dtcs(&r, 0x43, false));
    }
    if let Ok(r) = transport.send_command("07", Duration::from_millis(2000)) {
        for dtc in parse_dtcs(&r, 0x47, true) {
            if !all.iter().any(|existing| existing.code == dtc.code) {
                all.push(dtc);
            }
        }
    }
    all
}

#[cfg(target_os = "linux")]
mod bt_linux {
    use super::*;
    use dbus::{
        arg::{PropMap, RefArg, Variant},
        blocking::{Connection, stdintf::org_freedesktop_dbus::ObjectManager},
    };
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

    const BLE_PAIRS: &[(&str, &str)] = &[
        // Vgate iCar Pro BLE ("IOS-Vlink")
        (
            "00002af0-0000-1000-8000-00805f9b34fb",
            "00002af1-0000-1000-8000-00805f9b34fb",
        ),
        // Vgate / Veepeak / generic ELM327 BLE
        (
            "0000fff1-0000-1000-8000-00805f9b34fb",
            "0000fff2-0000-1000-8000-00805f9b34fb",
        ),
        // HM-10 / single-char UART ELM327 BLE
        (
            "0000ffe1-0000-1000-8000-00805f9b34fb",
            "0000ffe1-0000-1000-8000-00805f9b34fb",
        ),
        // Vgate vLinker / Nordic GATT
        (
            "bef8d6c9-9c21-4c9e-b632-bd58c1009f9f",
            "bef8d6c9-9c21-4c9e-b632-bd58c1009f9f",
        ),
    ];

    fn prop_str(props: &PropMap, key: &str) -> Option<String> {
        props.get(key)?.0.as_str().map(str::to_string)
    }

    fn prop_bool(props: &PropMap, key: &str) -> bool {
        props.get(key).and_then(|v| v.0.as_u64()).unwrap_or(0) != 0
    }

    fn prop_i16(props: &PropMap, key: &str) -> Option<i16> {
        props.get(key)?.0.as_i64().map(|v| v as i16)
    }

    pub fn scan_devices(duration: Duration) -> Result<Vec<BtDeviceInfo>, String> {
        let conn = Connection::new_system().map_err(|e| format!("D-Bus: {e}"))?;
        let adapter = conn.with_proxy("org.bluez", "/org/bluez/hci0", Duration::from_secs(5));
        let _started: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StartDiscovery", ());
        std::thread::sleep(duration);
        let _stopped: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StopDiscovery", ());

        let root = conn.with_proxy("org.bluez", "/", Duration::from_secs(5));
        let objs = root
            .get_managed_objects()
            .map_err(|e| format!("BlueZ objects: {e}"))?;
        let mut list = Vec::new();
        for (_path, ifaces) in objs {
            let Some(dev) = ifaces.get("org.bluez.Device1") else {
                continue;
            };
            let Some(mac) = prop_str(dev, "Address").and_then(|a| valid_mac(&a)) else {
                continue;
            };
            let name = prop_str(dev, "Name")
                .or_else(|| prop_str(dev, "Alias"))
                .unwrap_or_else(|| mac.clone());
            let likely_obd = is_likely_obd_name(&name);
            list.push(BtDeviceInfo {
                mac,
                name,
                rssi: prop_i16(dev, "RSSI"),
                paired: prop_bool(dev, "Paired"),
                connected: prop_bool(dev, "Connected"),
                likely_obd,
            });
        }
        list.sort_by(|a, b| {
            b.likely_obd
                .cmp(&a.likely_obd)
                .then_with(|| b.rssi.unwrap_or(-120).cmp(&a.rssi.unwrap_or(-120)))
        });
        Ok(list)
    }

    struct BleTransport {
        conn: Connection,
        write_path: String,
        write_without_resp: bool,
        notify_file: fs::File,
    }

    impl ElmTransport for BleTransport {
        fn transport_name(&self) -> &'static str {
            "ble"
        }

        fn send_command(&mut self, cmd: &str, timeout: Duration) -> Result<String, String> {
            drain_nonblocking(&mut self.notify_file);
            let payload = format!("{cmd}\r").into_bytes();
            let proxy = self
                .conn
                .with_proxy("org.bluez", &self.write_path, Duration::from_secs(3));
            let mut opts: HashMap<String, Variant<Box<dyn RefArg>>> = HashMap::new();
            if self.write_without_resp {
                opts.insert(
                    "type".into(),
                    Variant(Box::new("command".to_string()) as Box<dyn RefArg>),
                );
            }
            let res: Result<(), _> =
                proxy.method_call("org.bluez.GattCharacteristic1", "WriteValue", (payload, opts));
            res.map_err(|e| format!("BLE write: {e}"))?;
            read_until_prompt(&mut self.notify_file, timeout)
        }
    }

    struct RfcommTransport {
        file: fs::File,
    }

    impl ElmTransport for RfcommTransport {
        fn transport_name(&self) -> &'static str {
            "rfcomm"
        }

        fn send_command(&mut self, cmd: &str, timeout: Duration) -> Result<String, String> {
            drain_nonblocking(&mut self.file);
            let payload = format!("{cmd}\r");
            self.file
                .write_all(payload.as_bytes())
                .map_err(|e| format!("RFCOMM write: {e}"))?;
            read_until_prompt(&mut self.file, timeout)
        }
    }

    fn set_nonblocking(fd: i32) {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    fn drain_nonblocking(file: &mut fs::File) {
        let mut junk = [0u8; 256];
        while matches!(file.read(&mut junk), Ok(n) if n > 0) {}
    }

    fn read_until_prompt(file: &mut fs::File, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::with_capacity(256);
        let mut chunk = [0u8; 256];
        let fd = file.as_raw_fd();
        loop {
            let now = Instant::now();
            if now >= deadline {
                if !buf.is_empty() {
                    break;
                }
                return Err("Timeout réponse ELM327".into());
            }
            let rem_ms = deadline.saturating_duration_since(now).as_millis().min(200) as i32;
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let rc = unsafe { libc::poll(&mut pfd, 1, rem_ms) };
            if rc < 0 {
                continue;
            }
            if (pfd.revents & (libc::POLLERR | libc::POLLHUP)) != 0 && (pfd.revents & libc::POLLIN) == 0 {
                return Err("Connexion OBD fermée".into());
            }
            if rc > 0 {
                match file.read(&mut chunk) {
                    Ok(0) => return Err("Connexion OBD fermée (EOF)".into()),
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.contains(&b'>') {
                            break;
                        }
                        if buf.len() > 4096 {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(format!("Lecture OBD: {e}")),
                }
            }
        }
        let text = String::from_utf8_lossy(&buf)
            .replace('\r', "\n")
            .replace('>', "");
        Ok(text)
    }

    fn find_ble_chars(
        conn: &Connection,
        dev_path: &str,
    ) -> Result<Option<(String, String, bool)>, String> {
        let root = conn.with_proxy("org.bluez", "/", Duration::from_secs(5));
        let objs = root
            .get_managed_objects()
            .map_err(|e| format!("BlueZ GATT scan: {e}"))?;
        let mut by_uuid: HashMap<String, (String, Vec<String>)> = HashMap::new();
        for (path, ifaces) in objs {
            let pstr = path.to_string();
            if !pstr.starts_with(dev_path) {
                continue;
            }
            if let Some(ch) = ifaces.get("org.bluez.GattCharacteristic1") {
                if let Some(uuid) = prop_str(ch, "UUID") {
                    let mut flags = Vec::new();
                    if let Some(fvar) = ch.get("Flags") {
                        if let Some(iter) = fvar.0.as_iter() {
                            for item in iter {
                                if let Some(s) = item.as_str() {
                                    flags.push(s.to_string());
                                }
                            }
                        }
                    }
                    by_uuid.insert(uuid.to_ascii_lowercase(), (pstr, flags));
                }
            }
        }
        for &(notify_u, write_u) in BLE_PAIRS {
            if let (Some((np, _)), Some((wp, wflags))) = (by_uuid.get(notify_u), by_uuid.get(write_u)) {
                let no_resp = wflags.iter().any(|f| f == "write-without-response")
                    && !wflags.iter().any(|f| f == "write");
                return Ok(Some((np.clone(), wp.clone(), no_resp)));
            }
        }
        Ok(None)
    }

    fn connect_rfcomm(mac: &str) -> Result<Box<dyn ElmTransport>, String> {
        #[repr(C)]
        struct SockAddrRc {
            rc_family: u16,
            rc_bdaddr: [u8; 6],
            rc_channel: u8,
        }
        let parts: Vec<u8> = mac
            .split(':')
            .filter_map(|p| u8::from_str_radix(p, 16).ok())
            .collect();
        if parts.len() != 6 {
            return Err("MAC RFCOMM invalide".into());
        }
        let fd = unsafe { libc::socket(31, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 3) };
        if fd < 0 {
            return Err(format!("socket RFCOMM: {}", std::io::Error::last_os_error()));
        }
        let addr = SockAddrRc {
            rc_family: 31,
            rc_bdaddr: [parts[5], parts[4], parts[3], parts[2], parts[1], parts[0]],
            rc_channel: 1,
        };
        let rc = unsafe {
            libc::connect(
                fd,
                (&addr as *const SockAddrRc).cast::<libc::sockaddr>(),
                std::mem::size_of::<SockAddrRc>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(format!("RFCOMM connect: {err}"));
        }
        set_nonblocking(fd);
        let file = unsafe { fs::File::from_raw_fd(fd) };
        Ok(Box::new(RfcommTransport { file }))
    }

    pub fn connect_elm(mac: &str) -> Result<(Box<dyn ElmTransport>, String), String> {
        let conn = Connection::new_system().map_err(|e| format!("D-Bus: {e}"))?;
        let dev_path = format!("/org/bluez/hci0/dev_{}", mac.replace(':', "_"));
        let dev = conn.with_proxy("org.bluez", &dev_path, Duration::from_secs(10));

        // If not in BlueZ cache yet, run a brief discovery so BlueZ sees the advertisement.
        let connect_res: Result<(), _> = dev.method_call("org.bluez.Device1", "Connect", ());
        if connect_res.is_err() {
            let adapter = conn.with_proxy("org.bluez", "/org/bluez/hci0", Duration::from_secs(5));
            let _: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StartDiscovery", ());
            std::thread::sleep(Duration::from_secs(3));
            let _: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StopDiscovery", ());
            let retry: Result<(), _> = dev.method_call("org.bluez.Device1", "Connect", ());
            retry.map_err(|e| format!("Bluetooth connect ({mac}): {e}"))?;
        }

        // Wait up to 4 s for GATT ServicesResolved (BLE).
        let mut dev_name = mac.to_string();
        for _ in 0..20 {
            let root = conn.with_proxy("org.bluez", "/", Duration::from_secs(3));
            if let Ok(objs) = root.get_managed_objects() {
                if let Some(ifaces) = objs.get(&dbus::Path::new(&dev_path).unwrap()) {
                    if let Some(props) = ifaces.get("org.bluez.Device1") {
                        if let Some(n) = prop_str(props, "Name").or_else(|| prop_str(props, "Alias")) {
                            dev_name = n;
                        }
                        if prop_bool(props, "ServicesResolved") {
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        if let Some((notify_path, write_path, write_without_resp)) = find_ble_chars(&conn, &dev_path)? {
            let nproxy = conn.with_proxy("org.bluez", &notify_path, Duration::from_secs(5));
            let opts: HashMap<String, Variant<Box<dyn RefArg>>> = HashMap::new();
            let (fd, _mtu): (OwnedFd, u16) = nproxy
                .method_call("org.bluez.GattCharacteristic1", "AcquireNotify", (opts,))
                .map_err(|e| format!("BLE AcquireNotify: {e}"))?;
            set_nonblocking(fd.as_raw_fd());
            let notify_file = fs::File::from(fd);
            return Ok((
                Box::new(BleTransport {
                    conn,
                    write_path,
                    write_without_resp,
                    notify_file,
                }),
                dev_name,
            ));
        }

        // Fallback to Classic SPP RFCOMM (e.g., "Android-Vlink").
        let rfcomm = connect_rfcomm(mac)?;
        Ok((rfcomm, dev_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elm_mode01_and_supported_pid_bitmaps() {
        let rpm = parse_mode01_bytes("SEARCHING...\r\n41 0C 1A F8 \r\n>", 0x0C).unwrap();
        assert_eq!(rpm, vec![0x1A, 0xF8]);
        assert_eq!((u16::from(rpm[0]) * 256 + u16::from(rpm[1])) / 4, 1726);

        let bitmap = parse_mode01_bytes("4100BE3FA813\r\n", 0x00).unwrap();
        let pids = decode_supported_pids(0x00, &bitmap);
        assert!(pids.contains(&0x04));
        assert!(pids.contains(&0x05));
        assert!(pids.contains(&0x0C));
        assert!(pids.contains(&0x0D));
        assert!(pids.contains(&0x20));
    }

    #[test]
    fn decodes_fiat_multijet_dtcs_and_voltage() {
        assert_eq!(parse_at_rv("14.2V\r\n>"), Some(14.2));
        let dtcs = parse_dtcs("0: 43 02 04 01 24 63\r\n>", 0x43, false);
        assert_eq!(dtcs.len(), 2);
        assert_eq!(dtcs[0].code, "P0401");
        assert!(dtcs[0].description_fr.contains("EGR"));
        assert_eq!(dtcs[1].code, "P2463");
        assert_eq!(dtcs[1].severity, "critical");
    }

    #[test]
    fn estimates_diesel_fuel_rate_accurately() {
        // Direct PID 0x5E takes precedence.
        assert_eq!(
            estimate_diesel_fuel_lph(Some(4.8), Some(15.0), Some(30.0), None, None, None),
            Some(4.8)
        );
        // MAF + Load Diesel MultiJet fallback at ~35% load (AFR ~40.1), 20 g/s -> ~2.15 L/h.
        let est = estimate_diesel_fuel_lph(None, Some(20.0), Some(35.0), None, None, None).unwrap();
        assert!(est > 1.5 && est < 3.0, "unexpected diesel estimate: {est}");
    }

    #[test]
    fn recognizes_vgate_obd_names() {
        assert!(is_likely_obd_name("IOS-Vlink"));
        assert!(is_likely_obd_name("Android-Vlink"));
        assert!(is_likely_obd_name("Vgate iCar Pro"));
        assert!(!is_likely_obd_name("iPhone de Aymene"));
    }
}
