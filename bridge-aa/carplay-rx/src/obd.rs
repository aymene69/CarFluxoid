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
pub struct UdsRawEntry {
    pub did: String,
    pub label: String,
    pub hex: String,
    pub decoded: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UdsScanResult {
    pub scanned_at: u64,
    pub can_addressing: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecu_part_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecu_hw_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecu_sw_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecu_odometer_km: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oil_degradation_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oil_service_km_remaining: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oil_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_soot_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_diff_mbar: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_last_regen_km: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_avg_regen_km: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_regen_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_regen_progress_pct: Option<f64>,
    pub injectors_mm3: Vec<Option<f64>>,
    pub raw_dids: Vec<UdsRawEntry>,
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
    pub egr_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egr_error_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lambda: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pedal_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_soot_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpf_diff_kpa: Option<f64>,
    pub dpf_regen_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gear: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_hp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torque_nm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub g_lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub g_lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_s: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dist_since_clear_km: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmups_since_clear: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsFix>,
    pub dtcs: Vec<DtcCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uds: Option<UdsScanResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_trip: Option<TripSummary>,
    pub debug_log: Vec<String>,
    pub updated_at: u64,
}

#[derive(Clone, Debug)]
enum ObdCommand {
    Connect(String),
    Disconnect,
    ReadDtcs,
    ClearDtcs,
    FinishTrip,
    RunUdsScan,
}

struct ObdService {
    live: Mutex<ObdLiveSnapshot>,
    devices: Mutex<Vec<BtDeviceInfo>>,
    pending_cmd: Mutex<Option<ObdCommand>>,
    sim_until: Mutex<Option<Instant>>,
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
                sim_until: Mutex::new(None),
            })
        })
        .clone()
}

fn is_sim_active(svc: &ObdService) -> bool {
    let mut guard = svc.sim_until.lock().unwrap();
    match *guard {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

fn push_log(svc: &Arc<ObdService>, msg: impl Into<String>) {
    let text = msg.into();
    eprintln!("OBD_LOG {text}");
    let mut live = svc.live.lock().unwrap();
    live.debug_log.push(text);
    if live.debug_log.len() > 24 {
        let drop_n = live.debug_log.len() - 24;
        live.debug_log.drain(0..drop_n);
    }
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
        // Skip standalone ISO-TP multi-frame byte-count header lines like "014" or "00B"
        if !t.contains(':') && t.len() <= 3 && t.chars().all(|c| c.is_ascii_hexdigit()) {
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

/// Decode ISO 14229 UDS Service 19 02 (reportDTCByStatusMask) reply (59 02 <avail> [hi mid lo status]*).
pub fn parse_uds_1902(raw: &str) -> Vec<DtcCode> {
    let mut hex = String::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("NO DATA")
            || t.starts_with("SEARCHING")
            || t.starts_with("STOPPED")
            || t.starts_with("UNABLE")
            || t.starts_with("ERROR")
            || t.starts_with('?')
        {
            continue;
        }
        if !t.contains(':') && t.len() <= 3 && t.chars().all(|c| c.is_ascii_hexdigit()) {
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
    if hex.len() < 6 || !hex.len().is_multiple_of(2) {
        return Vec::new();
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    let Some(pos) = bytes.windows(2).position(|w| w[0] == 0x59 && w[1] == 0x02) else {
        return Vec::new();
    };
    if pos + 3 >= bytes.len() {
        return Vec::new();
    }
    let payload = &bytes[pos + 3..];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for rec in payload.chunks_exact(4) {
        if rec[0] == 0 && rec[1] == 0 {
            continue;
        }
        let prefix = match (rec[0] >> 6) & 0x03 {
            0 => 'P',
            1 => 'C',
            2 => 'B',
            _ => 'U',
        };
        let d1 = (rec[0] >> 4) & 0x03;
        let d2 = rec[0] & 0x0F;
        let d3 = (rec[1] >> 4) & 0x0F;
        let d4 = rec[1] & 0x0F;
        let code = format!("{prefix}{d1}{d2:X}{d3:X}{d4:X}");
        let status = rec[3];
        let confirmed = (status & 0x08) != 0;
        if seen.insert(code.clone()) {
            let (desc, sev) = describe_dtc_fr(&code);
            out.push(DtcCode {
                code,
                pending: !confirmed,
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

/// Estimate Diesel MultiJet fuel rate (L/h) from:
/// 1) Direct PID 0x5E,
/// 2) Exact Wideband O2 Lambda (PID 0x34) + MAF (PID 0x10) — laboratory-grade accuracy on Fiat MultiJet,
/// 3) Dynamic Load-based AFR + MAF (or MAP/RPM/IAT speed-density fallback).
pub fn estimate_diesel_fuel_lph(
    direct_fuel_rate_lph: Option<f64>,
    maf_gps: Option<f64>,
    lambda: Option<f64>,
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
    const DIESEL_STOICH_AFR: f64 = 14.5;
    let maf = maf_gps.or_else(|| {
        // Speed-density fallback for 1.6L MultiJet if MAF isn't exposed.
        let (map, r) = (map_kpa?, rpm?);
        if r < 300.0 {
            return Some(0.0);
        }
        let temp_k = (intake_c.unwrap_or(25.0) + 273.15).clamp(230.0, 380.0);
        let disp_l = 1.598;
        let ve = 0.88;
        let moles_per_s = (map * disp_l / (8.3145 * temp_k)) * (r / 120.0) * ve;
        Some((moles_per_s * 28.97).max(0.0))
    })?;
    let afr = if let Some(lam) = lambda.filter(|&l| (0.85..=12.0).contains(&l)) {
        if lam > 7.5 {
            // Overrun / deceleration fuel cut.
            return Some(0.0);
        }
        (DIESEL_STOICH_AFR * lam).clamp(14.0, 110.0)
    } else {
        let load = load_pct.unwrap_or(25.0).clamp(0.0, 100.0);
        // Diesel MultiJet runs lean at idle (~52:1) down to ~18:1 under full boost/load.
        (52.0 - 0.34 * load).clamp(18.0, 55.0)
    };
    let fuel_g_s = maf / afr;
    let lph = (fuel_g_s * 3.6) / DIESEL_DENSITY_KG_L;
    Some((lph.clamp(0.0, 60.0) * 100.0).round() / 100.0)
}

fn estimate_gear(speed_kmh: f64, rpm: f64) -> String {
    if speed_kmh < 2.5 || rpm < 600.0 {
        return "N".into();
    }
    let ratio = speed_kmh / (rpm / 1000.0);
    if ratio < 11.5 {
        "1".into()
    } else if ratio < 19.5 {
        "2".into()
    } else if ratio < 28.5 {
        "3".into()
    } else if ratio < 38.5 {
        "4".into()
    } else if ratio < 49.5 {
        "5".into()
    } else {
        "6".into()
    }
}

fn estimate_diesel_power_torque(fuel_lph: Option<f64>, rpm: Option<f64>) -> (Option<f64>, Option<f64>) {
    let Some(lph) = fuel_lph else {
        return (None, None);
    };
    // Diesel LHV ~9.96 kWh/L * ~36% effective thermal efficiency -> ~3.58 kW per L/h.
    let kw = (lph * 3.58).clamp(0.0, 125.0);
    let hp = (kw * 1.3596).round();
    let torque = rpm
        .filter(|&r| r >= 700.0)
        .map(|r| ((kw * 9549.3) / r).clamp(0.0, 420.0).round());
    (Some(hp), torque)
}

pub fn parse_uds_22(raw: &str, hi: u8, lo: u8) -> Option<Vec<u8>> {
    let mut hex = String::with_capacity(raw.len());
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("SEARCHING")
            || t.starts_with("NO DATA")
            || t.starts_with("STOPPED")
            || t.starts_with("UNABLE")
            || t.starts_with("ERROR")
            || t.starts_with('?')
        {
            continue;
        }
        // Skip standalone ISO-TP multi-frame byte-count header lines like "014" or "00B"
        if !t.contains(':') && t.len() <= 3 && t.chars().all(|c| c.is_ascii_hexdigit()) {
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
    if hex.len() < 6 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let s = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(s, 16).ok()?);
    }
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i] == 0x62 && bytes[i + 1] == hi && bytes[i + 2] == lo {
            return Some(bytes[i + 3..].to_vec());
        }
    }
    None
}

pub fn parse_vin(raw: &str) -> Option<String> {
    let mut hex = String::with_capacity(raw.len());
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("SEARCHING")
            || t.starts_with("NO DATA")
            || t.starts_with("ERROR")
            || t.starts_with('?')
        {
            continue;
        }
        // Skip ISO-TP length header line like "014"
        if t.len() <= 3 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        // Skip standalone ISO-TP multi-frame byte-count header lines like "014" or "00B"
        if !t.contains(':') && t.len() <= 3 && t.chars().all(|c| c.is_ascii_hexdigit()) {
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
    if hex.len() < 12 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let s = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(s, 16).ok()?);
    }
    let start = (0..bytes.len().saturating_sub(2))
        .find(|&i| bytes[i] == 0x49 && bytes[i + 1] == 0x02)?;
    let mut payload = &bytes[start + 2..];
    if !payload.is_empty() && payload[0] <= 5 {
        payload = &payload[1..];
    }
    let vin: String = payload
        .iter()
        .copied()
        .filter(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        .map(char::from)
        .take(17)
        .collect();
    if vin.len() == 17 {
        Some(vin)
    } else {
        None
    }
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
    let path = bridge_dir().join("video.sock.gps.json");
    if let Ok(meta) = fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = modified.elapsed() {
                if age > Duration::from_secs(12) {
                    return None;
                }
            }
        }
    }
    let text = fs::read_to_string(&path).ok()?;
    let fix: GpsFix = serde_json::from_str(&text).ok()?;
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
    started_instant: Instant,
    last_active_at: Instant,
    last_sample_at: Instant,
    last_checkpoint_at: Instant,
    was_running: bool,
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
    last_runtime_s: Option<u32>,
    has_gps: bool,
    has_obd: bool,
}

impl ActiveTrip {
    fn start(now: u64, live: &ObdLiveSnapshot) -> Self {
        let now_i = Instant::now();
        // Guarantee a unique trip ID even if clock hasn't synced yet after reboot.
        let dir = trips_dir();
        let mut ts = now;
        while dir.join(format!("trip-{ts}.json")).exists() {
            ts = ts.saturating_add(1);
        }
        Self {
            id: format!("trip-{ts}"),
            started_at: ts,
            started_instant: now_i,
            last_active_at: now_i,
            last_sample_at: now_i - Duration::from_secs(2),
            last_checkpoint_at: now_i,
            was_running: true,
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
            last_runtime_s: live.runtime_s,
            has_gps: live.gps.is_some(),
            has_obd: live.connected,
        }
    }

    /// If the trip began before the Pi clock was synced (1970 epoch), rebase its start
    /// timestamp and file ID once real UTC time arrives from GPS or the iPhone app.
    fn rebase_clock_if_needed(&mut self) {
        let now = now_unix_secs();
        if self.started_at < 1_700_000_000 && now >= 1_700_000_000 {
            let old_path = trips_dir().join(format!("{}.json", self.id));
            let real_start = now.saturating_sub(self.started_instant.elapsed().as_secs());
            self.started_at = real_start;
            self.id = format!("trip-{real_start}");
            let _ = fs::remove_file(old_path);
            self.save();
        }
    }

    fn ingest(&mut self, live: &ObdLiveSnapshot) {
        let now_i = Instant::now();
        let dt_s = now_i
            .saturating_duration_since(self.last_sample_at)
            .as_secs_f64()
            .clamp(0.1, 5.0);
        let spd = if live.connected {
            live.speed_kmh
                .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
        } else {
            live.gps.as_ref().and_then(|g| g.speed_kmh)
        }
        .unwrap_or(0.0)
        .max(0.0);
        let rpm = if live.connected {
            live.rpm.unwrap_or(0.0).max(0.0)
        } else {
            0.0
        };

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

        let elapsed_s = self.started_instant.elapsed().as_secs();
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
        let duration_s = self
            .last_active_at
            .saturating_duration_since(self.started_instant)
            .as_secs()
            .max(1);
        let ended_at = self.started_at.saturating_add(duration_s);
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
            if let Ok(mut f) = fs::File::create(&tmp) {
                if f.write_all(json.as_bytes()).is_ok() && f.sync_all().is_ok() {
                    if fs::rename(&tmp, &dst).is_ok() {
                        if let Ok(d) = fs::File::open(&dir) {
                            let _ = d.sync_all();
                        }
                    }
                }
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

pub fn request_finish_trip() {
    let svc = service();
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::FinishTrip);
}

pub fn request_uds_scan() -> String {
    let svc = service();
    if is_sim_active(&svc) {
        let mut live = svc.live.lock().unwrap();
        let sim_uds = UdsScanResult {
            scanned_at: now_unix_secs(),
            can_addressing: "29-bit FCA (18DA10F1 · SIM)".into(),
            ecu_part_number: Some("55275498".into()),
            ecu_hw_number: Some("BOSCH EDC17C69".into()),
            ecu_sw_version: Some("1037558214".into()),
            ecu_odometer_km: Some(85_420),
            oil_degradation_pct: Some(74.0),
            oil_service_km_remaining: Some(11_200),
            oil_temp_c: Some(91.0),
            dpf_soot_pct: live.dpf_soot_pct.or(Some(36.4)),
            dpf_temp_c: live.dpf_temp_c.or(Some(312.0)),
            dpf_diff_mbar: Some(28.0),
            dpf_last_regen_km: Some(412),
            dpf_avg_regen_km: Some(540),
            dpf_regen_count: Some(158),
            dpf_regen_progress_pct: Some(if live.dpf_regen_active { 62.0 } else { 0.0 }),
            injectors_mm3: vec![Some(-0.24), Some(0.18), Some(-0.11), Some(0.31)],
            raw_dids: vec![
                UdsRawEntry { did: "F187".into(), label: "Réf. pièce ECU".into(), hex: "3535323735343938".into(), decoded: "55275498".into() },
                UdsRawEntry { did: "18E4".into(), label: "Colmatage FAP".into(), hex: "094A".into(), decoded: "36.4 %".into() },
                UdsRawEntry { did: "2001".into(), label: "Odomètre ECU".into(), hex: "014DAC".into(), decoded: "85420 km".into() },
                UdsRawEntry { did: "3812".into(), label: "Qualité huile".into(), hex: "4A".into(), decoded: "74 %".into() },
            ],
        };
        live.uds = Some(sim_uds);
        live.oil_c = Some(91.0);
        return serde_json::to_string(&*live).unwrap_or_else(|_| "{}".into());
    }
    *svc.pending_cmd.lock().unwrap() = Some(ObdCommand::RunUdsScan);
    live_json()
}

#[derive(Deserialize)]
struct SimPayload {
    rpm: Option<f64>,
    speed_kmh: Option<f64>,
    coolant_c: Option<f64>,
    intake_c: Option<f64>,
    load_pct: Option<f64>,
    throttle_pct: Option<f64>,
    pedal_pct: Option<f64>,
    map_kpa: Option<f64>,
    baro_kpa: Option<f64>,
    boost_bar: Option<f64>,
    maf_gps: Option<f64>,
    rail_bar: Option<f64>,
    fuel_level_pct: Option<f64>,
    fuel_rate_lph: Option<f64>,
    fuel_l_per_100km: Option<f64>,
    battery_v: Option<f64>,
    egr_pct: Option<f64>,
    egr_error_pct: Option<f64>,
    lambda: Option<f64>,
    dpf_temp_c: Option<f64>,
    dpf_soot_pct: Option<f64>,
    dpf_diff_kpa: Option<f64>,
    dpf_regen_active: Option<bool>,
    gear: Option<String>,
    power_hp: Option<f64>,
    torque_nm: Option<f64>,
    g_lon: Option<f64>,
    g_lat: Option<f64>,
    dist_since_clear_km: Option<u32>,
    warmups_since_clear: Option<u16>,
}

pub fn apply_simulated_json(body: &[u8]) -> Result<String, String> {
    if body.len() > 16 * 1024 {
        return Err("Payload simulation trop grand.".into());
    }
    let sim: SimPayload = serde_json::from_slice(body).map_err(|e| format!("JSON invalide: {e}"))?;
    let svc = service();
    *svc.sim_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs(15));
    let mut live = svc.live.lock().unwrap();
    live.connected = true;
    live.state = "simulated".into();
    live.error = None;
    live.adapter_name = Some("Simulateur OBD-II".into());
    live.transport = "sim".into();
    live.elm_version = Some("CarFluxoid Virtual ELM327".into());
    live.protocol = Some("AUTO, ISO 15765-4 (SIM)".into());
    if sim.rpm.is_some() { live.rpm = sim.rpm; }
    if sim.speed_kmh.is_some() { live.speed_kmh = sim.speed_kmh; }
    if sim.coolant_c.is_some() { live.coolant_c = sim.coolant_c; }
    if sim.intake_c.is_some() { live.intake_c = sim.intake_c; }
    if sim.load_pct.is_some() { live.load_pct = sim.load_pct; }
    if sim.throttle_pct.is_some() { live.throttle_pct = sim.throttle_pct; }
    if sim.pedal_pct.is_some() { live.pedal_pct = sim.pedal_pct; }
    if sim.map_kpa.is_some() { live.map_kpa = sim.map_kpa; }
    if sim.baro_kpa.is_some() { live.baro_kpa = sim.baro_kpa; }
    if sim.boost_bar.is_some() { live.boost_bar = sim.boost_bar; }
    if sim.maf_gps.is_some() { live.maf_gps = sim.maf_gps; }
    if sim.rail_bar.is_some() { live.rail_bar = sim.rail_bar; }
    if sim.fuel_level_pct.is_some() { live.fuel_level_pct = sim.fuel_level_pct; }
    if sim.fuel_rate_lph.is_some() { live.fuel_rate_lph = sim.fuel_rate_lph; }
    if sim.fuel_l_per_100km.is_some() { live.fuel_l_per_100km = sim.fuel_l_per_100km; }
    if sim.battery_v.is_some() { live.battery_v = sim.battery_v; }
    if sim.egr_pct.is_some() { live.egr_pct = sim.egr_pct; }
    if sim.egr_error_pct.is_some() { live.egr_error_pct = sim.egr_error_pct; }
    if sim.lambda.is_some() { live.lambda = sim.lambda; }
    if sim.dpf_temp_c.is_some() { live.dpf_temp_c = sim.dpf_temp_c; }
    if sim.dpf_soot_pct.is_some() { live.dpf_soot_pct = sim.dpf_soot_pct; }
    if sim.dpf_diff_kpa.is_some() { live.dpf_diff_kpa = sim.dpf_diff_kpa; }
    if let Some(regen) = sim.dpf_regen_active { live.dpf_regen_active = regen; }
    if sim.gear.is_some() { live.gear = sim.gear; }
    if sim.power_hp.is_some() { live.power_hp = sim.power_hp; }
    if sim.torque_nm.is_some() { live.torque_nm = sim.torque_nm; }
    if sim.g_lon.is_some() { live.g_lon = sim.g_lon; }
    if sim.g_lat.is_some() { live.g_lat = sim.g_lat; }
    if sim.dist_since_clear_km.is_some() { live.dist_since_clear_km = sim.dist_since_clear_km; }
    if sim.warmups_since_clear.is_some() { live.warmups_since_clear = sim.warmups_since_clear; }
    live.updated_at = now_unix_secs();
    write_dashboard_obd_snapshot(&live);
    serde_json::to_string(&*live).map_err(|e| e.to_string())
}

pub fn stop_simulation() -> String {
    let svc = service();
    *svc.sim_until.lock().unwrap() = None;
    let mut live = svc.live.lock().unwrap();
    let has_mac = live.adapter_mac.is_some();
    live.connected = false;
    live.state = if has_mac { "connecting".into() } else { "disconnected".into() };
    live.adapter_name = None;
    live.transport = "none".into();
    live.error = None;
    live.rpm = None;
    live.speed_kmh = None;
    live.gear = None;
    live.boost_bar = None;
    live.power_hp = None;
    live.torque_nm = None;
    live.dpf_regen_active = false;
    live.updated_at = now_unix_secs();
    write_dashboard_obd_snapshot(&live);
    serde_json::to_string(&*live).unwrap_or_else(|_| "{}".into())
}

pub fn sync_system_time(unix_secs: u64) -> Result<u64, String> {
    if !(1_700_000_000..=4_102_444_800).contains(&unix_secs) {
        return Err("Timestamp Unix hors plage.".into());
    }
    let cur = now_unix_secs();
    if cur.abs_diff(unix_secs) >= 2 {
        #[cfg(target_os = "linux")]
        {
            let ts = libc::timespec {
                tv_sec: unix_secs as libc::time_t,
                tv_nsec: 0,
            };
            let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) };
            if rc == 0 {
                let svc = service();
                push_log(&svc, format!("Horloge Pi synchronisée ({unix_secs})"));
            }
        }
    }
    Ok(now_unix_secs())
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
        if is_sim_active(&svc) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        // Always update GPS & trip logger even when no OBD adapter is paired/connected.
        let gps = read_car_gps();
        {
            let mut live = svc.live.lock().unwrap();
            live.gps = gps.clone();
            live.updated_at = now_unix_secs();
            update_trip_state(&mut active_trip, &mut live);
            write_dashboard_obd_snapshot(&live);
        }

        // Check pending disconnect/connect/finish command.
        if let Some(cmd) = svc.pending_cmd.lock().unwrap().take() {
            match cmd {
                ObdCommand::Disconnect => {
                    if let Some(trip) = active_trip.take() {
                        trip.save();
                        svc.live.lock().unwrap().active_trip = None;
                    }
                    continue;
                }
                ObdCommand::Connect(mac) => {
                    last_reconnect = Instant::now() - Duration::from_secs(60);
                    svc.live.lock().unwrap().adapter_mac = Some(mac);
                }
                ObdCommand::FinishTrip => {
                    if let Some(mut trip) = active_trip.take() {
                        trip.rebase_clock_if_needed();
                        trip.save();
                        svc.live.lock().unwrap().active_trip = None;
                        push_log(&svc, format!("Trajet terminé manuellement ({:.2} km)", trip.distance_km));
                    }
                }
                ObdCommand::ReadDtcs | ObdCommand::ClearDtcs | ObdCommand::RunUdsScan => {}
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
            match bt_linux::connect_elm(&svc, &mac) {
                Ok((mut transport, dev_name)) => {
                    push_log(
                        &svc,
                        format!("Connecté {dev_name} ({})", transport.transport_name()),
                    );
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
                        // Clear stale live driving metrics so parked cars never look "engine running".
                        live.rpm = None;
                        live.speed_kmh = None;
                        live.fuel_rate_lph = None;
                        live.fuel_l_per_100km = None;
                        live.boost_bar = None;
                        live.gear = None;
                        live.power_hp = None;
                        live.torque_nm = None;
                        live.runtime_s = None;
                        if live.adapter_mac.is_some() {
                            live.state = "reconnecting".into();
                        } else {
                            live.state = "disconnected".into();
                        }
                        write_dashboard_obd_snapshot(&live);
                    }
                }
                Err(err) => {
                    push_log(&svc, format!("Erreur BT: {err}"));
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
    // Only trust OBD RPM/speed while the ECU link is actually live; otherwise rely solely on fresh GPS (<12 s).
    let spd = if live.connected {
        live.speed_kmh
            .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
            .unwrap_or(0.0)
    } else {
        live.gps.as_ref().and_then(|g| g.speed_kmh).unwrap_or(0.0)
    };
    let rpm = if live.connected {
        live.rpm.unwrap_or(0.0)
    } else {
        0.0
    };
    let moving_or_running = spd >= 2.0 || rpm >= 450.0;

    // Hardware engine-restart detection via PID 011F (runtime since engine start):
    // if runtime dropped significantly, the driver turned off the ignition and restarted for a new trip!
    if let (Some(trip), Some(cur_rt)) = (active_trip.as_ref(), live.runtime_s) {
        if let Some(prev_rt) = trip.last_runtime_s {
            if prev_rt >= 20 && cur_rt.saturating_add(15) < prev_rt {
                eprintln!(
                    "OBD_TRIP_SPLIT_RESTART id={} prev_rt={prev_rt}s cur_rt={cur_rt}s km={:.2}",
                    trip.id, trip.distance_km
                );
                trip.save();
                *active_trip = None;
                live.active_trip = None;
            }
        }
    }

    if active_trip.is_none() && moving_or_running {
        let trip = ActiveTrip::start(now_unix_secs(), live);
        eprintln!("OBD_TRIP_START id={}", trip.id);
        *active_trip = Some(trip);
    }

    if let Some(trip) = active_trip.as_mut() {
        trip.rebase_clock_if_needed();
        if let Some(rt) = live.runtime_s {
            trip.last_runtime_s = Some(rt);
        }
        trip.ingest(live);

        // Save immediately if engine just stopped (so power loss never loses the end of a trip)
        // or every 10 seconds during driving.
        let just_stopped = trip.was_running && !moving_or_running;
        trip.was_running = moving_or_running;
        if just_stopped || trip.last_checkpoint_at.elapsed() >= Duration::from_secs(10) {
            trip.save();
            trip.last_checkpoint_at = Instant::now();
        }
        let summary = trip.summary();
        live.active_trip = Some(summary);

        // Auto-finalize faster (20 s) when ECU/OBD link dropped (ignition OFF),
        // or after 60 s when still connected with engine off & vehicle stopped (tolerates Start&Stop).
        let idle_timeout = if live.connected {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(20)
        };
        if trip.last_active_at.elapsed() >= idle_timeout {
            eprintln!("OBD_TRIP_END id={} km={:.2}", trip.id, trip.distance_km);
            trip.save();
            *active_trip = None;
            live.active_trip = None;
        }
    }
}

#[cfg(target_os = "linux")]
fn discover_supported_pids(
    svc: &Arc<ObdService>,
    transport: &mut dyn ElmTransport,
    supported: &mut BTreeSet<u8>,
    first_timeout: Duration,
) -> bool {
    let mut any_ok = false;
    for base in [0x00u8, 0x20, 0x40, 0x60, 0x80] {
        let cmd = format!("01{:02X}", base);
        let tmo = if base == 0x00 {
            first_timeout
        } else {
            Duration::from_millis(2500)
        };
        let resp = match transport.send_command(&cmd, tmo) {
            Ok(r) => r,
            Err(e) => {
                push_log(svc, format!("{cmd} -> {e}"));
                break;
            }
        };
        let summary: String = resp
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        push_log(svc, format!("{cmd} -> {summary}"));
        let Some(data) = parse_mode01_bytes(&resp, base) else {
            break;
        };
        any_ok = true;
        let pids = decode_supported_pids(base, &data);
        let has_next = pids.contains(&(base + 0x20));
        supported.extend(pids);
        if !has_next {
            break;
        }
    }
    any_ok
}

#[cfg(target_os = "linux")]
fn uds_bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("")
}

#[cfg(target_os = "linux")]
fn uds_bytes_to_ascii(bytes: &[u8]) -> Option<String> {
    let s: String = bytes
        .iter()
        .copied()
        .filter(|b| (0x20..=0x7E).contains(b))
        .map(|b| b as char)
        .collect::<String>()
        .trim()
        .to_string();
    if s.len() >= 3 { Some(s) } else { None }
}

#[cfg(target_os = "linux")]
fn uds_u16(b: &[u8]) -> Option<u16> {
    if b.len() >= 2 { Some(u16::from(b[0]) * 256 + u16::from(b[1])) } else { None }
}

#[cfg(target_os = "linux")]
fn uds_u24_or_u32(b: &[u8]) -> Option<u32> {
    if b.len() >= 4 {
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    } else if b.len() == 3 {
        Some((u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]))
    } else if b.len() == 2 {
        Some(u32::from(u16::from(b[0]) * 256 + u16::from(b[1])))
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn uds_i16(b: &[u8]) -> Option<i16> {
    if b.len() >= 2 { Some(i16::from_be_bytes([b[0], b[1]])) } else { None }
}

#[cfg(target_os = "linux")]
fn summarize_elm_raw(raw: &str) -> String {
    let s: String = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if s.len() > 96 {
        format!("{}…", &s[..96])
    } else if s.is_empty() {
        "(vide)".into()
    } else {
        s
    }
}

/// Switch ELM327 explicitly to 29-bit 500 kbps FCA/Fiat CAN (`ATSP7`) with
/// physical addressing to Engine ECU `18DA10F1` -> `18DAF110` and explicit
/// ISO-TP Flow Control (required on Fiat Tipo 2017 / EDC17C69 / MJD 9DF even
/// when standard Mode 01 OBD-II negotiated 11-bit CAN `ATSP6`).
#[cfg(target_os = "linux")]
fn enter_fca_uds_29bit(svc: &Arc<ObdService>, t: &mut dyn ElmTransport, verbose: bool) {
    let sp = t.send_command("ATSP7", Duration::from_millis(800));
    let _ = t.send_command("ATCP18", Duration::from_millis(500));
    let _ = t.send_command("ATSHDA10F1", Duration::from_millis(600));
    let _ = t.send_command("ATCRA18DAF110", Duration::from_millis(600));
    let _ = t.send_command("ATFCSH18DA10F1", Duration::from_millis(600));
    let _ = t.send_command("ATFCSD300000", Duration::from_millis(500));
    let _ = t.send_command("ATFCSM1", Duration::from_millis(500));
    let sess = t.send_command("1003", Duration::from_millis(700));
    if verbose {
        push_log(
            svc,
            format!(
                "UDS 29b ATSP7={} 1003={}",
                sp.as_deref().map(summarize_elm_raw).unwrap_or_else(|e| e.clone()),
                sess.as_deref().map(summarize_elm_raw).unwrap_or_else(|e| e.clone())
            ),
        );
    }
}

#[cfg(target_os = "linux")]
fn enter_uds_11bit(svc: &Arc<ObdService>, t: &mut dyn ElmTransport, verbose: bool) {
    let sp = t.send_command("ATSP6", Duration::from_millis(800));
    let _ = t.send_command("ATCRA7E8", Duration::from_millis(500));
    let _ = t.send_command("ATSH7E0", Duration::from_millis(600));
    let _ = t.send_command("ATFCSH7E0", Duration::from_millis(500));
    let _ = t.send_command("ATFCSD300000", Duration::from_millis(500));
    let _ = t.send_command("ATFCSM1", Duration::from_millis(500));
    let sess = t.send_command("1003", Duration::from_millis(700));
    if verbose {
        push_log(
            svc,
            format!(
                "UDS 11b ATSP6={} 1003={}",
                sp.as_deref().map(summarize_elm_raw).unwrap_or_else(|e| e.clone()),
                sess.as_deref().map(summarize_elm_raw).unwrap_or_else(|e| e.clone())
            ),
        );
    }
}

#[cfg(target_os = "linux")]
fn restore_obd_standard(t: &mut dyn ElmTransport, default_29bit: bool) {
    let _ = t.send_command("ATCRA", Duration::from_millis(500));
    let _ = t.send_command("ATFCSM0", Duration::from_millis(500));
    if default_29bit {
        let _ = t.send_command("ATSP7", Duration::from_millis(600));
        let _ = t.send_command("ATCP18", Duration::from_millis(500));
        let _ = t.send_command("ATSHDB33F1", Duration::from_millis(600));
    } else {
        let _ = t.send_command("ATSP6", Duration::from_millis(600));
        let _ = t.send_command("ATSH7DF", Duration::from_millis(600));
    }
}

#[cfg(target_os = "linux")]
fn run_uds_multijet_scan(
    svc: &Arc<ObdService>,
    transport: &mut dyn ElmTransport,
    default_29bit: bool,
) -> (UdsScanResult, Vec<DtcCode>) {
    push_log(svc, "Scan UDS MultiJet Pro (ATSP7 29-bit FCA + Service 22)…");

    let query_logged = |t: &mut dyn ElmTransport, hi: u8, lo: u8, log_raw: bool| -> Option<Vec<u8>> {
        let cmd = format!("22{hi:02X}{lo:02X}");
        let resp = match t.send_command(&cmd, Duration::from_millis(1200)) {
            Ok(r) => r,
            Err(e) => {
                if log_raw {
                    push_log(svc, format!("UDS {cmd} -> ERR {e}"));
                }
                return None;
            }
        };
        let parsed = parse_uds_22(&resp, hi, lo);
        if log_raw {
            push_log(svc, format!("UDS {cmd} -> {}", summarize_elm_raw(&resp)));
        }
        parsed
    };
    let query = |t: &mut dyn ElmTransport, hi: u8, lo: u8| -> Option<Vec<u8>> {
        query_logged(t, hi, lo, false)
    };

    // Fiat/FCA MultiJet (Tipo 356, 500X, Giulietta) exposes UDS on 29-bit 18DA10F1
    // even when standard Mode 01 OBD-II negotiated 11-bit CAN (ATSP6). Try 29-bit FIRST!
    let mut use_29 = true;
    enter_fca_uds_29bit(svc, transport, true);
    let mut part_bytes = query_logged(transport, 0xF1, 0x87, true);
    let mut soot_probe = query_logged(transport, 0x18, 0xE4, true);
    let mut temp_probe = query_logged(transport, 0x18, 0xDE, true);
    if part_bytes.is_none() && soot_probe.is_none() && temp_probe.is_none() {
        // Fallback to 11-bit physical ECU addressing (7E0 -> 7E8)
        use_29 = false;
        enter_uds_11bit(svc, transport, true);
        part_bytes = query_logged(transport, 0xF1, 0x87, true);
        soot_probe = query_logged(transport, 0x18, 0xE4, true);
        temp_probe = query_logged(transport, 0x18, 0xDE, true);
    }

    let mut res = UdsScanResult {
        scanned_at: now_unix_secs(),
        can_addressing: if use_29 {
            "29-bit FCA (ATSP7 · 18DA10F1)".into()
        } else {
            "11-bit CAN (ATSP6 · 7E0)".into()
        },
        injectors_mm3: vec![None, None, None, None],
        ..Default::default()
    };

    let mut record = |did: &str, label: &str, bytes: &[u8], decoded: String| {
        res.raw_dids.push(UdsRawEntry {
            did: did.into(),
            label: label.into(),
            hex: uds_bytes_to_hex(bytes),
            decoded,
        });
    };

    // 1. ECU Identification (ISO 14229 standard DIDs)
    if let Some(ref b) = part_bytes {
        if let Some(s) = uds_bytes_to_ascii(b) {
            res.ecu_part_number = Some(s.clone());
            record("F187", "Réf. pièce ECU", b, s);
        }
    }
    for (hi, lo, label) in [(0xF1, 0x91, "Matériel ECU"), (0xF1, 0x8A, "Fournisseur ECU")] {
        if res.ecu_hw_number.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                if let Some(s) = uds_bytes_to_ascii(&b) {
                    res.ecu_hw_number = Some(s.clone());
                    record(&format!("{hi:02X}{lo:02X}"), label, &b, s);
                }
            }
        }
    }
    for (hi, lo, label) in [(0xF1, 0x94, "Logiciel ECU"), (0xF1, 0x95, "Version SW ECU")] {
        if res.ecu_sw_version.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                if let Some(s) = uds_bytes_to_ascii(&b) {
                    res.ecu_sw_version = Some(s.clone());
                    record(&format!("{hi:02X}{lo:02X}"), label, &b, s);
                }
            }
        }
    }

    // 2. ECU Odometer (FCA EDC17C69 / MJD 8F3/9DF)
    for (hi, lo) in [(0x20, 0x01), (0x29, 0x5A), (0x10, 0x02)] {
        if res.ecu_odometer_km.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                if let Some(raw) = uds_u24_or_u32(&b) {
                    let km = if (500..=600_000).contains(&raw) {
                        Some(raw)
                    } else if (5_000..=6_000_000).contains(&raw) {
                        Some(raw / 10)
                    } else {
                        None
                    };
                    if let Some(k) = km {
                        res.ecu_odometer_km = Some(k);
                        record(&format!("{hi:02X}{lo:02X}"), "Odomètre ECU", &b, format!("{k} km"));
                    } else {
                        record(&format!("{hi:02X}{lo:02X}"), "Odomètre brut", &b, format!("{raw}"));
                    }
                }
            }
        }
    }

    // 3. FAP / DPF Health (soot, temp, diff pressure, regen counters & intervals)
    if let Some(ref b) = soot_probe {
        if let Some(raw) = uds_u16(b) {
            let pct = ((f64::from(raw) * 1000.0 / 65535.0) * 10.0).round() / 10.0;
            if (0.0..=150.0).contains(&pct) {
                res.dpf_soot_pct = Some(pct);
                record("18E4", "Colmatage FAP", b, format!("{pct:.1} %"));
            }
        } else if !b.is_empty() {
            let pct = f64::from(b[0]);
            if (0.0..=150.0).contains(&pct) {
                res.dpf_soot_pct = Some(pct);
                record("18E4", "Colmatage FAP", b, format!("{pct:.0} %"));
            }
        }
    }
    if res.dpf_soot_pct.is_none() {
        // Fallback DPF soot DIDs on some MJD 9DF / EDC17 SW versions
        for (hi, lo) in [(0x18, 0xE3), (0x10, 0x0B)] {
            if let Some(b) = query(transport, hi, lo) {
                let pct = if let Some(raw) = uds_u16(&b) {
                    let v = ((f64::from(raw) * 1000.0 / 65535.0) * 10.0).round() / 10.0;
                    if (0.0..=150.0).contains(&v) { Some(v) } else { None }
                } else if !b.is_empty() {
                    let v = f64::from(b[0]);
                    if (0.0..=150.0).contains(&v) { Some(v) } else { None }
                } else { None };
                if let Some(p) = pct {
                    res.dpf_soot_pct = Some(p);
                    record(&format!("{hi:02X}{lo:02X}"), "Colmatage FAP", &b, format!("{p:.1} %"));
                    break;
                }
            }
        }
    }
    if let Some(b) = temp_probe.or_else(|| query(transport, 0x18, 0xDF)) {
        if let Some(raw) = uds_u16(&b) {
            let c = (f64::from(raw) * 0.02 - 40.0).round();
            if (-40.0..=1100.0).contains(&c) {
                res.dpf_temp_c = Some(c);
                record("18DE", "Température FAP", &b, format!("{c:.0} °C"));
            }
        }
    }
    if let Some(b) = query(transport, 0x18, 0xE0) {
        if let Some(raw) = uds_i16(&b) {
            let mbar = f64::from(raw);
            if (-50.0..=1200.0).contains(&mbar) {
                res.dpf_diff_mbar = Some(mbar);
                record("18E0", "Pression diff. FAP", &b, format!("{mbar:.0} mbar"));
            }
        }
    }
    if let Some(b) = query(transport, 0x18, 0xE5) {
        if let Some(raw) = uds_u24_or_u32(&b) {
            let km = if raw <= 20_000 { raw } else { raw / 10 };
            res.dpf_last_regen_km = Some(km);
            record("18E5", "Dist. depuis régén FAP", &b, format!("{km} km"));
        }
    }
    if let Some(b) = query(transport, 0x18, 0xE6) {
        if let Some(raw) = uds_u24_or_u32(&b) {
            let km = if raw <= 20_000 { raw } else { raw / 10 };
            res.dpf_avg_regen_km = Some(km);
            record("18E6", "Moyenne entre régén FAP", &b, format!("{km} km"));
        }
    }
    if let Some(b) = query(transport, 0x18, 0xE8) {
        if let Some(raw) = uds_u16(&b) {
            res.dpf_regen_count = Some(u32::from(raw));
            record("18E8", "Nb régénérations FAP", &b, format!("{raw}"));
        }
    }
    if let Some(b) = query(transport, 0x18, 0xA4) {
        if let Some(raw) = uds_u16(&b) {
            let pct = ((f64::from(raw) * 100.0 / 65535.0) * 10.0).round() / 10.0;
            res.dpf_regen_progress_pct = Some(pct);
            record("18A4", "Avancement régén FAP", &b, format!("{pct:.1} %"));
        } else if !b.is_empty() {
            let pct = f64::from(b[0]).min(100.0);
            res.dpf_regen_progress_pct = Some(pct);
            record("18A4", "Avancement régén FAP", &b, format!("{pct:.0} %"));
        }
    }

    // 4. Oil Degradation, Oil Service km, & Oil Temp
    for (hi, lo) in [(0x38, 0x12), (0x18, 0xE9)] {
        if res.oil_degradation_pct.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                let pct = if b.len() == 1 {
                    Some(f64::from(b[0]))
                } else if let Some(raw) = uds_u16(&b) {
                    if raw <= 100 { Some(f64::from(raw)) } else { Some(((f64::from(raw) * 100.0 / 65535.0) * 10.0).round() / 10.0) }
                } else { None };
                if let Some(p) = pct.filter(|p| (0.0..=100.0).contains(p)) {
                    res.oil_degradation_pct = Some(p);
                    record(&format!("{hi:02X}{lo:02X}"), "Dégradation huile", &b, format!("{p:.0} %"));
                }
            }
        }
    }
    for (hi, lo) in [(0x38, 0x13), (0x20, 0x04)] {
        if res.oil_service_km_remaining.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                if let Some(raw) = uds_u24_or_u32(&b) {
                    if raw <= 60_000 {
                        res.oil_service_km_remaining = Some(raw);
                        record(&format!("{hi:02X}{lo:02X}"), "Km avant vidange", &b, format!("{raw} km"));
                    }
                }
            }
        }
    }
    for (hi, lo) in [(0x19, 0x4F), (0x10, 0x09)] {
        if res.oil_temp_c.is_none() {
            if let Some(b) = query(transport, hi, lo) {
                let c = if let Some(raw) = uds_u16(&b) {
                    let v1 = (f64::from(raw) * 0.02 - 40.0).round();
                    let v2 = (f64::from(raw) * 0.1 - 40.0).round();
                    if (-20.0..=155.0).contains(&v1) { Some(v1) } else if (-20.0..=155.0).contains(&v2) { Some(v2) } else { None }
                } else if b.len() == 1 {
                    let v = f64::from(b[0]) - 40.0;
                    if (-20.0..=155.0).contains(&v) { Some(v) } else { None }
                } else { None };
                if let Some(temp) = c {
                    res.oil_temp_c = Some(temp);
                    record(&format!("{hi:02X}{lo:02X}"), "Température huile", &b, format!("{temp:.0} °C"));
                }
            }
        }
    }

    // 5. Diesel MultiJet 4-Cylinder Injector Corrections (mm³/stroke)
    let mut got_inj = false;
    for (idx, lo) in [0x35u8, 0x36, 0x37, 0x38].iter().enumerate() {
        if let Some(b) = query(transport, 0x19, *lo) {
            if let Some(raw) = uds_i16(&b) {
                let val = ((f64::from(raw) * 0.01) * 100.0).round() / 100.0;
                if (-8.0..=8.0).contains(&val) {
                    res.injectors_mm3[idx] = Some(val);
                    got_inj = true;
                    record(&format!("19{lo:02X}"), &format!("Injecteur Cyl. {}", idx + 1), &b, format!("{val:+.2} mm³"));
                }
            }
        }
    }
    if !got_inj {
        // Fallback for Magneti Marelli MJD 8F3 injector DIDs (18B0..18B3)
        for (idx, lo) in [0xB0u8, 0xB1, 0xB2, 0xB3].iter().enumerate() {
            if let Some(b) = query(transport, 0x18, *lo) {
                if let Some(raw) = uds_i16(&b) {
                    let val = ((f64::from(raw) * 0.01) * 100.0).round() / 100.0;
                    if (-8.0..=8.0).contains(&val) {
                        res.injectors_mm3[idx] = Some(val);
                        record(&format!("18{lo:02X}"), &format!("Injecteur Cyl. {}", idx + 1), &b, format!("{val:+.2} mm³"));
                    }
                }
            }
        }
    }

    // 6. Read UDS Manufacturer DTCs (Service 19 02 0F = active + pending + confirmed)
    let uds_dtcs = match transport.send_command("19020F", Duration::from_millis(2000)) {
        Ok(r) => {
            let parsed = parse_uds_1902(&r);
            push_log(
                svc,
                format!("UDS DTC 19020F -> {} ({} codes)", summarize_elm_raw(&r), parsed.len()),
            );
            parsed
        }
        Err(e) => {
            push_log(svc, format!("UDS DTC 19020F -> ERR {e}"));
            Vec::new()
        }
    };

    restore_obd_standard(transport, default_29bit);
    push_log(
        svc,
        format!(
            "Scan UDS terminé : {} DIDs lus, {} DTC ({})",
            res.raw_dids.len(),
            uds_dtcs.len(),
            res.can_addressing
        ),
    );
    (res, uds_dtcs)
}

#[cfg(target_os = "linux")]
fn run_elm_session(
    svc: &Arc<ObdService>,
    transport: &mut dyn ElmTransport,
    active_trip: &mut Option<ActiveTrip>,
) {
    std::thread::sleep(Duration::from_millis(250));
    let init_cmds = ["ATZ", "ATE0", "ATL0", "ATS0", "ATH0", "ATAT1", "ATSP0"];
    let mut elm_ver = None;
    for cmd in init_cmds {
        let timeout = if cmd == "ATZ" {
            Duration::from_millis(3000)
        } else {
            Duration::from_millis(1500)
        };
        let mut res = transport.send_command(cmd, timeout);
        if res.is_err() && cmd == "ATZ" {
            std::thread::sleep(Duration::from_millis(400));
            res = transport.send_command(cmd, timeout);
        }
        match res {
            Ok(resp) => {
                let one: String = resp
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && *l != cmd)
                    .collect::<Vec<_>>()
                    .join(" ");
                push_log(
                    svc,
                    format!("{cmd} -> {}", if one.is_empty() { "OK" } else { &one }),
                );
                if cmd == "ATZ" {
                    elm_ver = if one.is_empty() {
                        Some("ELM327 / Vgate".into())
                    } else {
                        Some(one)
                    };
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            Err(e) => {
                push_log(svc, format!("Init {cmd} ERR: {e}"));
                svc.live.lock().unwrap().error = Some(format!("Init {cmd}: {e}"));
                return;
            }
        }
    }

    // Discover supported Mode 01 PIDs (0100..0160). Allow up to 8s on ATSP0 SEARCHING...,
    // then explicitly try ATSP6 (CAN 11/500) and ATSP7 (CAN 29/500 Fiat/FCA) if needed.
    let mut supported: BTreeSet<u8> = BTreeSet::new();
    let mut ecu_ok = discover_supported_pids(svc, transport, &mut supported, Duration::from_secs(8));
    if !ecu_ok {
        for proto in ["ATSP6", "ATSP7"] {
            push_log(svc, format!("Essai protocole {proto}..."));
            let _ = transport.send_command(proto, Duration::from_millis(1500));
            ecu_ok =
                discover_supported_pids(svc, transport, &mut supported, Duration::from_secs(5));
            if ecu_ok {
                break;
            }
        }
    }

    let protocol = transport
        .send_command("ATDP", Duration::from_millis(1200))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let supported_labels: Vec<String> = supported.iter().map(|p| format!("01{p:02X}")).collect();
    push_log(
        svc,
        format!(
            "Prêt: {} ({} PIDs)",
            protocol.as_deref().unwrap_or("CAN"),
            supported_labels.len()
        ),
    );

    // Initial VIN (Mode 09 PID 02) & DTC read.
    let vin = transport
        .send_command("0902", Duration::from_millis(2500))
        .ok()
        .and_then(|r| parse_vin(&r));
    if let Some(ref v) = vin {
        push_log(svc, format!("VIN détecté: {}***{}", &v[..4], &v[13..]));
    }
    let is_29bit_can = protocol
        .as_deref()
        .map(|p| p.contains("29"))
        .unwrap_or(false);
    let mut dtcs = read_all_dtcs(svc, transport, is_29bit_can);

    // Probe ELM327 fast-return digit (`010C1` = return immediately after 1 CAN frame instead of waiting ATST).
    let fast_suffix: &'static str = match transport.send_command("010C1", Duration::from_millis(800)) {
        Ok(r) if parse_mode01_bytes(&r, 0x0C).is_some_and(|d| d.len() >= 2) => {
            push_log(svc, "Mode rapide ELM327 activé (01xx1 -> ~8-10 Hz)");
            "1"
        }
        _ => "",
    };

    {
        let mut live = svc.live.lock().unwrap();
        live.connected = true;
        live.state = "connected".into();
        live.error = if ecu_ok {
            None
        } else {
            Some("ELM327 OK, mais calculateur muet (contact mis ?)".into())
        };
        live.elm_version = elm_ver;
        live.protocol = protocol;
        live.vin = vin;
        live.supported_pids = supported_labels;
        live.dtcs = dtcs.clone();
    }

    let mut probed_failures: HashMap<u8, u8> = HashMap::new();
    let mut uds_dpf_failures: u8 = 0;
    let mut uds_use_29: Option<bool> = None;
    let mut uds_probe_count: u32 = 0;
    let mut fast_rot_idx: usize = 0;
    let mut slow_rot_idx: usize = 0;
    let mut last_slow_step = Instant::now() - Duration::from_secs(5);
    let mut last_uds = Instant::now() - Duration::from_secs(60);
    let mut consec_errors = 0u32;
    let mut prev_g_at: Option<Instant> = None;
    let mut prev_g_spd: f64 = 0.0;
    let mut prev_g_bearing: Option<f64> = None;
    let mut smooth_g_lon: f64 = 0.0;
    let mut smooth_g_lat: f64 = 0.0;

    // Secondary fast PIDs rotated 1 per cycle so RPM + Speed stay at ~8-10 Hz!
    const FAST_ROT_PIDS: [u8; 7] = [0x04, 0x0B, 0x10, 0x49, 0x34, 0x11, 0x5E];
    // Slow PIDs spread 1 per ~350 ms so RPM/Speed NEVER freezes for 1.5 s! (0xFF = AT RV battery voltage)
    const SLOW_ROT_PIDS: [u8; 17] = [
        0x05, 0x2F, 0x23, 0x42, 0xFF, 0x0F, 0x5C, 0x33, 0x46, 0x2C, 0x2D, 0x30, 0x3C, 0x78, 0x7A,
        0x1F, 0x31,
    ];

    loop {
        if is_sim_active(svc) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        if let Some(cmd) = svc.pending_cmd.lock().unwrap().take() {
            match cmd {
                ObdCommand::Disconnect => return,
                ObdCommand::Connect(new_mac) => {
                    svc.live.lock().unwrap().adapter_mac = Some(new_mac);
                    return;
                }
                ObdCommand::ReadDtcs => {
                    dtcs = read_all_dtcs(svc, transport, is_29bit_can);
                    let mut live = svc.live.lock().unwrap();
                    live.dtcs = dtcs.clone();
                    write_dashboard_obd_snapshot(&live);
                }
                ObdCommand::ClearDtcs => {
                    push_log(svc, "Effacement DTC Mode 04 demandé…");
                    let _ = transport.send_command("04", Duration::from_millis(2000));
                    std::thread::sleep(Duration::from_millis(400));
                    dtcs = read_all_dtcs(svc, transport, is_29bit_can);
                    let mut live = svc.live.lock().unwrap();
                    live.dtcs = dtcs.clone();
                    write_dashboard_obd_snapshot(&live);
                }
                ObdCommand::FinishTrip => {
                    if let Some(mut trip) = active_trip.take() {
                        trip.rebase_clock_if_needed();
                        trip.save();
                        svc.live.lock().unwrap().active_trip = None;
                        push_log(svc, format!("Trajet terminé manuellement ({:.2} km)", trip.distance_km));
                    }
                }
                ObdCommand::RunUdsScan => {
                    let (uds, uds_dtcs) = run_uds_multijet_scan(svc, transport, is_29bit_can);
                    let mut live = svc.live.lock().unwrap();
                    if let Some(oil) = uds.oil_temp_c {
                        live.oil_c = Some(oil);
                    }
                    if let Some(soot) = uds.dpf_soot_pct {
                        live.dpf_soot_pct = Some(soot);
                    }
                    if let Some(temp) = uds.dpf_temp_c {
                        live.dpf_temp_c = Some(temp);
                    }
                    if let Some(mbar) = uds.dpf_diff_mbar {
                        live.dpf_diff_kpa = Some((mbar * 0.1 * 10.0).round() / 10.0);
                    }
                    for d in uds_dtcs {
                        if !live.dtcs.iter().any(|existing| existing.code == d.code) {
                            live.dtcs.push(d);
                        }
                    }
                    live.uds = Some(uds);
                    write_dashboard_obd_snapshot(&live);
                }
            }
        }

        let should_poll = |pid: u8, sup: &BTreeSet<u8>, fails: &HashMap<u8, u8>| -> bool {
            sup.contains(&pid) || fails.get(&pid).copied().unwrap_or(0) < 2
        };
        let mark_probe = |pid: u8, ok: bool, sup: &mut BTreeSet<u8>, fails: &mut HashMap<u8, u8>| {
            if ok {
                sup.insert(pid);
                fails.insert(pid, 0);
            } else if !sup.contains(&pid) {
                *fails.entry(pid).or_insert(0) += 1;
            }
        };

        let mut ok_this_cycle = false;
        let mut rpm = None;
        let mut speed = None;
        let mut load = None;
        let mut map = None;
        let mut throttle = None;
        let mut maf = None;
        let mut direct_fuel_lph = None;
        let mut coolant = None;
        let mut oil = None;
        let mut intake = None;
        let mut baro = None;
        let mut rail_bar = None;
        let mut fuel_lvl = None;
        let mut ambient = None;
        let mut batt_v = None;
        let mut egr_pct = None;
        let mut egr_error_pct = None;
        let mut lambda = None;
        let mut pedal_pct = None;
        let mut dpf_temp_c = None;
        let mut dpf_soot_pct = None;
        let mut dpf_diff_kpa = None;
        let mut runtime_s = None;
        let mut dist_since_clear_km = None;
        let mut warmups_since_clear = None;

        // Priority tier (EVERY cycle -> ~8-10 Hz): RPM (010C) + Speed (010D).
        if should_poll(0x0C, &supported, &probed_failures) {
            let mut got = false;
            let cmd = format!("010C{fast_suffix}");
            if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(650)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x0C) {
                    if d.len() >= 2 {
                        rpm = Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 4.0);
                        ok_this_cycle = true;
                        got = true;
                    }
                }
            }
            mark_probe(0x0C, got, &mut supported, &mut probed_failures);
        }
        if should_poll(0x0D, &supported, &probed_failures) {
            let mut got = false;
            let cmd = format!("010D{fast_suffix}");
            if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(650)) {
                if let Some(d) = parse_mode01_bytes(&r, 0x0D) {
                    if !d.is_empty() {
                        speed = Some(f64::from(d[0]));
                        ok_this_cycle = true;
                        got = true;
                    }
                }
            }
            mark_probe(0x0D, got, &mut supported, &mut probed_failures);
        }

        // Rotate ONE secondary fast PID per cycle (Load, MAP, MAF, Pedal, Lambda, Throttle, FuelRate).
        for _ in 0..FAST_ROT_PIDS.len() {
            let pid = FAST_ROT_PIDS[fast_rot_idx % FAST_ROT_PIDS.len()];
            fast_rot_idx = (fast_rot_idx + 1) % FAST_ROT_PIDS.len();
            if !should_poll(pid, &supported, &probed_failures) {
                continue;
            }
            let cmd = format!("01{pid:02X}{fast_suffix}");
            let mut got = false;
            if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(650)) {
                if let Some(d) = parse_mode01_bytes(&r, pid) {
                    match pid {
                        0x04 if !d.is_empty() => {
                            load = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                            ok_this_cycle = true;
                            got = true;
                        }
                        0x0B if !d.is_empty() => {
                            map = Some(f64::from(d[0]));
                            ok_this_cycle = true;
                            got = true;
                        }
                        0x10 if d.len() >= 2 => {
                            maf = Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 100.0);
                            ok_this_cycle = true;
                            got = true;
                        }
                        0x11 if !d.is_empty() => {
                            throttle = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                            ok_this_cycle = true;
                            got = true;
                        }
                        0x34 if d.len() >= 2 => {
                            let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                            let lam = (2.0 / 65536.0) * f64::from(raw);
                            if (0.5..=16.0).contains(&lam) {
                                lambda = Some((lam * 100.0).round() / 100.0);
                                ok_this_cycle = true;
                                got = true;
                            }
                        }
                        0x49 if !d.is_empty() => {
                            pedal_pct = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                            ok_this_cycle = true;
                            got = true;
                        }
                        0x5E if d.len() >= 2 => {
                            direct_fuel_lph =
                                Some(((u16::from(d[0]) * 256 + u16::from(d[1])) as f64) / 20.0);
                            ok_this_cycle = true;
                            got = true;
                        }
                        _ => {}
                    }
                }
            }
            mark_probe(pid, got, &mut supported, &mut probed_failures);
            break;
        }

        // Spread slow tier: poll ONE slow PID every 350 ms (eliminates 1.5 s stutters on Perf!).
        if last_slow_step.elapsed() >= Duration::from_millis(350) {
            last_slow_step = Instant::now();
            for _ in 0..SLOW_ROT_PIDS.len() {
                let pid = SLOW_ROT_PIDS[slow_rot_idx % SLOW_ROT_PIDS.len()];
                slow_rot_idx = (slow_rot_idx + 1) % SLOW_ROT_PIDS.len();
                if pid == 0xFF {
                    // Fallback battery voltage via ELM327 AT RV if PID 0142 isn't supported.
                    if supported.contains(&0x42) {
                        continue;
                    }
                    if let Ok(r) = transport.send_command("AT RV", Duration::from_millis(800)) {
                        batt_v = parse_at_rv(&r);
                        if batt_v.is_some() {
                            ok_this_cycle = true;
                        }
                    }
                    break;
                }
                if !should_poll(pid, &supported, &probed_failures) {
                    continue;
                }
                let is_multi = pid == 0x78 || pid == 0x7A;
                let cmd = if is_multi {
                    format!("01{pid:02X}")
                } else {
                    format!("01{pid:02X}{fast_suffix}")
                };
                let mut got = false;
                if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(900)) {
                    if let Some(d) = parse_mode01_bytes(&r, pid) {
                        match pid {
                            0x05 if !d.is_empty() => {
                                coolant = Some(f64::from(d[0]) - 40.0);
                                ok_this_cycle = true;
                                got = true;
                            }
                            0x0F if !d.is_empty() => {
                                intake = Some(f64::from(d[0]) - 40.0);
                                got = true;
                            }
                            0x5C if !d.is_empty() => {
                                oil = Some(f64::from(d[0]) - 40.0);
                                got = true;
                            }
                            0x33 if !d.is_empty() => {
                                baro = Some(f64::from(d[0]));
                                got = true;
                            }
                            0x23 if d.len() >= 2 => {
                                let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                                rail_bar = Some((f64::from(raw) / 10.0).round());
                                got = true;
                            }
                            0x2F if !d.is_empty() => {
                                fuel_lvl = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                                got = true;
                            }
                            0x46 if !d.is_empty() => {
                                ambient = Some(f64::from(d[0]) - 40.0);
                                got = true;
                            }
                            0x42 if d.len() >= 2 => {
                                let mv = u16::from(d[0]) * 256 + u16::from(d[1]);
                                batt_v = Some((f64::from(mv) / 10.0).round() / 100.0);
                                got = true;
                            }
                            0x2C if !d.is_empty() => {
                                egr_pct = Some((f64::from(d[0]) * 100.0 / 255.0 * 10.0).round() / 10.0);
                                got = true;
                            }
                            0x2D if !d.is_empty() => {
                                let err = ((f64::from(d[0]) - 128.0) * 100.0 / 128.0 * 10.0).round() / 10.0;
                                egr_error_pct = Some(err);
                                got = true;
                            }
                            0x30 if !d.is_empty() => {
                                warmups_since_clear = Some(u16::from(d[0]));
                                got = true;
                            }
                            0x3C if d.len() >= 2 => {
                                let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                                let c = (f64::from(raw) / 10.0 - 40.0).round();
                                if (-40.0..=1100.0).contains(&c) {
                                    dpf_temp_c = Some(c);
                                    got = true;
                                }
                            }
                            0x78 if d.len() >= 3 => {
                                let raw = u16::from(d[1]) * 256 + u16::from(d[2]);
                                let c = (f64::from(raw) / 10.0 - 40.0).round();
                                if (-40.0..=1100.0).contains(&c) {
                                    dpf_temp_c = Some(c);
                                    got = true;
                                }
                            }
                            0x7A if d.len() >= 3 => {
                                let raw = u16::from(d[1]) * 256 + u16::from(d[2]);
                                dpf_diff_kpa = Some((f64::from(raw) * 0.01 * 10.0).round() / 10.0);
                                got = true;
                            }
                            0x1F if d.len() >= 2 => {
                                runtime_s = Some(u32::from(u16::from(d[0]) * 256 + u16::from(d[1])));
                                got = true;
                            }
                            0x31 if d.len() >= 2 => {
                                dist_since_clear_km = Some(u32::from(u16::from(d[0]) * 256 + u16::from(d[1])));
                                got = true;
                            }
                            _ => {}
                        }
                    }
                }
                mark_probe(pid, got, &mut supported, &mut probed_failures);
                if slow_rot_idx == 0 {
                    svc.live.lock().unwrap().supported_pids =
                        supported.iter().map(|p| format!("01{p:02X}")).collect();
                }
                break;
            }
        }

        // Fiat MultiJet UDS Mode 22 DPF & Oil probe (explicit ATSP7 29-bit FCA -> 18DA10F1,
        // fallback 11-bit 7E0; every 30 s, stops after 3 misses).
        if uds_dpf_failures < 3 && last_uds.elapsed() >= Duration::from_secs(30) {
            last_uds = Instant::now();
            let verbose = uds_probe_count == 0;
            uds_probe_count = uds_probe_count.saturating_add(1);

            let modes: &[bool] = match uds_use_29 {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[true, false],
            };

            let mut uds_ok = false;
            for &try_29 in modes {
                if try_29 {
                    enter_fca_uds_29bit(svc, transport, verbose);
                } else {
                    enter_uds_11bit(svc, transport, verbose);
                }

                // 1. DPF Soot % (18E4 primary, 18E3 fallback)
                for (hi, lo) in [(0x18u8, 0xE4u8), (0x18, 0xE3)] {
                    if dpf_soot_pct.is_some() {
                        break;
                    }
                    let cmd = format!("22{hi:02X}{lo:02X}");
                    if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(1100)) {
                        if verbose {
                            push_log(svc, format!("FAP {cmd} ({}) -> {}", if try_29 { "29b" } else { "11b" }, summarize_elm_raw(&r)));
                        }
                        if let Some(d) = parse_uds_22(&r, hi, lo) {
                            if d.len() >= 2 {
                                let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                                let pct = ((f64::from(raw) * 1000.0 / 65535.0) * 10.0).round() / 10.0;
                                if (0.0..=150.0).contains(&pct) {
                                    dpf_soot_pct = Some(pct);
                                    uds_ok = true;
                                }
                            } else if !d.is_empty() {
                                let pct = f64::from(d[0]);
                                if (0.0..=150.0).contains(&pct) {
                                    dpf_soot_pct = Some(pct);
                                    uds_ok = true;
                                }
                            }
                        }
                    }
                }

                // 2. DPF Temperature °C (18DE primary, 18DF fallback)
                for (hi, lo) in [(0x18u8, 0xDEu8), (0x18, 0xDF)] {
                    if dpf_temp_c.is_some() {
                        break;
                    }
                    let cmd = format!("22{hi:02X}{lo:02X}");
                    if let Ok(r) = transport.send_command(&cmd, Duration::from_millis(1100)) {
                        if verbose {
                            push_log(svc, format!("FAP {cmd} ({}) -> {}", if try_29 { "29b" } else { "11b" }, summarize_elm_raw(&r)));
                        }
                        if let Some(d) = parse_uds_22(&r, hi, lo) {
                            if d.len() >= 2 {
                                let raw = u16::from(d[0]) * 256 + u16::from(d[1]);
                                let c = (f64::from(raw) * 0.02 - 40.0).round();
                                if (-40.0..=1100.0).contains(&c) {
                                    dpf_temp_c = Some(c);
                                    uds_ok = true;
                                }
                            }
                        }
                    }
                }

                // 3. DPF Diff Pressure (18E0, mbar -> kPa) & Oil Temp (194F) when UDS link responded
                if uds_ok {
                    if let Ok(r) = transport.send_command("2218E0", Duration::from_millis(1000)) {
                        if let Some(d) = parse_uds_22(&r, 0x18, 0xE0) {
                            if let Some(raw) = uds_i16(&d) {
                                let mbar = f64::from(raw);
                                if (-50.0..=1200.0).contains(&mbar) {
                                    dpf_diff_kpa = Some((mbar * 0.1 * 10.0).round() / 10.0);
                                }
                            }
                        }
                    }
                    if !supported.contains(&0x5C) {
                        if let Ok(r) = transport.send_command("22194F", Duration::from_millis(1000)) {
                            if let Some(d) = parse_uds_22(&r, 0x19, 0x4F) {
                                if let Some(raw) = uds_u16(&d) {
                                    let v1 = (f64::from(raw) * 0.02 - 40.0).round();
                                    let v2 = (f64::from(raw) * 0.1 - 40.0).round();
                                    if (-20.0..=155.0).contains(&v1) {
                                        oil = Some(v1);
                                    } else if (-20.0..=155.0).contains(&v2) {
                                        oil = Some(v2);
                                    }
                                }
                            }
                        }
                    }
                    uds_use_29 = Some(try_29);
                    break;
                }
            }
            restore_obd_standard(transport, is_29bit_can);

            if uds_ok {
                uds_dpf_failures = 0;
                if verbose {
                    push_log(
                        svc,
                        format!(
                            "FAP UDS actif ({}): suie={:?}% temp={:?}°C",
                            if uds_use_29 == Some(true) { "29b FCA" } else { "11b" },
                            dpf_soot_pct,
                            dpf_temp_c
                        ),
                    );
                }
            } else {
                uds_dpf_failures = uds_dpf_failures.saturating_add(1);
                push_log(svc, format!("FAP UDS sans réponse (essai {uds_dpf_failures}/3)"));
            }
        }

        if ok_this_cycle {
            consec_errors = 0;
        } else {
            consec_errors += 1;
            if consec_errors >= 3 {
                // ECU stopped answering (ignition cut): zero out stale RPM/speed right away
                // so the trip auto-finalizer starts counting down immediately instead of holding 820 rpm!
                let mut live = svc.live.lock().unwrap();
                live.rpm = Some(0.0);
                live.speed_kmh = Some(0.0);
                live.fuel_rate_lph = None;
                live.fuel_l_per_100km = None;
                live.gear = Some("N".into());
                live.power_hp = Some(0.0);
                live.torque_nm = Some(0.0);
            }
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
            if egr_pct.is_some() {
                live.egr_pct = egr_pct;
            }
            if egr_error_pct.is_some() {
                live.egr_error_pct = egr_error_pct;
            }
            if lambda.is_some() {
                live.lambda = lambda;
            }
            if pedal_pct.is_some() {
                live.pedal_pct = pedal_pct;
            }
            if dpf_temp_c.is_some() {
                live.dpf_temp_c = dpf_temp_c;
            }
            if dpf_soot_pct.is_some() {
                live.dpf_soot_pct = dpf_soot_pct;
            }
            if dpf_diff_kpa.is_some() {
                live.dpf_diff_kpa = dpf_diff_kpa;
            }
            if let Some(t) = live.dpf_temp_c {
                if t >= 480.0 {
                    live.dpf_regen_active = true;
                } else if t < 420.0 {
                    live.dpf_regen_active = false;
                }
            }
            if runtime_s.is_some() {
                live.runtime_s = runtime_s;
            }
            if dist_since_clear_km.is_some() {
                live.dist_since_clear_km = dist_since_clear_km;
            }
            if warmups_since_clear.is_some() {
                live.warmups_since_clear = warmups_since_clear;
            }
            if let Some(m) = live.map_kpa {
                let b = live.baro_kpa.unwrap_or(101.3);
                live.boost_bar = Some(((m - b) / 100.0 * 100.0).round() / 100.0);
            }
            let lph = estimate_diesel_fuel_lph(
                direct_fuel_lph,
                live.maf_gps,
                live.lambda,
                live.load_pct,
                live.map_kpa,
                live.rpm,
                live.intake_c,
            );
            live.fuel_rate_lph = lph;
            let (hp, nm) = estimate_diesel_power_torque(lph, live.rpm);
            live.power_hp = hp;
            live.torque_nm = nm;
            let spd_for_cons = live
                .speed_kmh
                .or_else(|| live.gps.as_ref().and_then(|g| g.speed_kmh))
                .unwrap_or(0.0);
            if let Some(r) = live.rpm {
                live.gear = Some(estimate_gear(spd_for_cons, r));
            }
            live.fuel_l_per_100km = if spd_for_cons >= 5.0 {
                lph.map(|r| ((r / spd_for_cons * 100.0).min(45.0) * 10.0).round() / 10.0)
            } else {
                None
            };
            // Estimate 2D G-force: longitudinal from OBD speed derivative, lateral from Uconnect GPS bearing rate
            let g_now = Instant::now();
            if let Some(prev_t) = prev_g_at {
                let dt = g_now.saturating_duration_since(prev_t).as_secs_f64();
                if (0.08..=2.5).contains(&dt) {
                    let dv_ms = (spd_for_cons - prev_g_spd) / 3.6;
                    let inst_lon = (dv_ms / (dt * 9.81)).clamp(-1.3, 1.3);
                    smooth_g_lon = smooth_g_lon * 0.55 + inst_lon * 0.45;
                    if let (Some(cur_b), Some(prev_b)) = (live.gps.as_ref().and_then(|g| g.bearing), prev_g_bearing) {
                        if spd_for_cons >= 10.0 {
                            let mut ddeg = cur_b - prev_b;
                            if ddeg > 180.0 { ddeg -= 360.0; }
                            if ddeg < -180.0 { ddeg += 360.0; }
                            let omega = ddeg.to_radians() / dt;
                            let v_ms = spd_for_cons / 3.6;
                            // Right turn (positive bearing delta) pushes G-ball left (negative g_lat)
                            let inst_lat = (-(v_ms * omega) / 9.81).clamp(-1.3, 1.3);
                            smooth_g_lat = smooth_g_lat * 0.60 + inst_lat * 0.40;
                        } else {
                            smooth_g_lat *= 0.6;
                        }
                    } else {
                        smooth_g_lat *= 0.8;
                    }
                }
            }
            prev_g_at = Some(g_now);
            prev_g_spd = spd_for_cons;
            prev_g_bearing = live.gps.as_ref().and_then(|g| g.bearing);
            live.g_lon = Some((smooth_g_lon * 1000.0).round() / 1000.0);
            live.g_lat = Some((smooth_g_lat * 1000.0).round() / 1000.0);
            live.updated_at = now_unix_secs();
            update_trip_state(active_trip, &mut live);
            write_dashboard_obd_snapshot(&live);
        }

        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(target_os = "linux")]
fn read_all_dtcs(
    svc: &Arc<ObdService>,
    transport: &mut dyn ElmTransport,
    default_29bit: bool,
) -> Vec<DtcCode> {
    let mut all = Vec::new();
    match transport.send_command("03", Duration::from_millis(2000)) {
        Ok(r) => {
            let found = parse_dtcs(&r, 0x43, false);
            push_log(
                svc,
                format!("DTC Mode 03 -> {} ({} confirmés)", summarize_elm_raw(&r), found.len()),
            );
            all.extend(found);
        }
        Err(e) => push_log(svc, format!("DTC Mode 03 -> ERR {e}")),
    }
    match transport.send_command("07", Duration::from_millis(2000)) {
        Ok(r) => {
            let found = parse_dtcs(&r, 0x47, true);
            push_log(
                svc,
                format!("DTC Mode 07 -> {} ({} en attente)", summarize_elm_raw(&r), found.len()),
            );
            for dtc in found {
                if !all.iter().any(|existing| existing.code == dtc.code) {
                    all.push(dtc);
                }
            }
        }
        Err(e) => push_log(svc, format!("DTC Mode 07 -> ERR {e}")),
    }
    // Also read Fiat/FCA 29-bit UDS Service 19 02 0F (catches manufacturer DTCs even if MIL is off)
    enter_fca_uds_29bit(svc, transport, false);
    if let Ok(r) = transport.send_command("19020F", Duration::from_millis(1800)) {
        let uds_found = parse_uds_1902(&r);
        push_log(
            svc,
            format!("DTC UDS 19020F -> {} ({} codes)", summarize_elm_raw(&r), uds_found.len()),
        );
        for dtc in uds_found {
            if !all.iter().any(|existing| existing.code == dtc.code) {
                all.push(dtc);
            }
        }
    }
    restore_obd_standard(transport, default_29bit);
    all
}

#[cfg(target_os = "linux")]
mod bt_linux {
    use super::*;
    use dbus::{
        arg::{PropMap, RefArg, Variant},
        blocking::{
            Connection,
            stdintf::org_freedesktop_dbus::{ObjectManager, Properties, PropertiesPropertiesChanged},
        },
        channel::Token,
        message::MatchRule,
    };
    use std::os::unix::io::{AsRawFd, FromRawFd};

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

    fn run_discovery(conn: &Connection, transport: &str, duration: Duration) {
        let adapter = conn.with_proxy("org.bluez", "/org/bluez/hci0", Duration::from_secs(5));
        let _ = adapter.set("org.bluez.Adapter1", "Powered", true);
        let mut filter: HashMap<String, Variant<Box<dyn RefArg>>> = HashMap::new();
        filter.insert(
            "Transport".into(),
            Variant(Box::new(transport.to_string()) as Box<dyn RefArg>),
        );
        let _: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "SetDiscoveryFilter", (filter,));
        let _: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StartDiscovery", ());
        std::thread::sleep(duration);
        let _: Result<(), _> = adapter.method_call("org.bluez.Adapter1", "StopDiscovery", ());
        // Allow Broadcom CYW43436 (Pi Zero 2 W) radio to exit scan mode cleanly before Connect().
        std::thread::sleep(Duration::from_millis(400));
    }

    pub fn scan_devices(duration: Duration) -> Result<Vec<BtDeviceInfo>, String> {
        let conn = Connection::new_system().map_err(|e| format!("D-Bus: {e}"))?;
        // LE scan catches Vgate iCar Pro ("IOS-Vlink") reliably even while Classic BT is active.
        run_discovery(&conn, "le", duration.min(Duration::from_secs(5)));

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

    struct BleNotifyTransport {
        conn: Connection,
        notify_path: String,
        write_path: String,
        write_without_resp: bool,
        rx_buf: Arc<Mutex<Vec<u8>>>,
        _match_token: Token,
    }

    impl Drop for BleNotifyTransport {
        fn drop(&mut self) {
            let nproxy = self
                .conn
                .with_proxy("org.bluez", &self.notify_path, Duration::from_secs(2));
            let _: Result<(), _> = nproxy.method_call("org.bluez.GattCharacteristic1", "StopNotify", ());
        }
    }

    impl BleNotifyTransport {
        fn write_raw(&mut self, payload: Vec<u8>) -> Result<(), String> {
            let proxy = self
                .conn
                .with_proxy("org.bluez", &self.write_path, Duration::from_secs(3));
            let try_write = |without_resp: bool| -> Result<(), dbus::Error> {
                let mut opts: HashMap<String, Variant<Box<dyn RefArg>>> = HashMap::new();
                opts.insert(
                    "type".into(),
                    Variant(
                        Box::new(if without_resp { "command" } else { "request" }.to_string())
                            as Box<dyn RefArg>,
                    ),
                );
                proxy.method_call(
                    "org.bluez.GattCharacteristic1",
                    "WriteValue",
                    (payload.clone(), opts),
                )
            };
            match try_write(self.write_without_resp) {
                Ok(()) => Ok(()),
                Err(first_err) => {
                    // Retry with alternate write type (some Vgate chips only accept one).
                    if try_write(!self.write_without_resp).is_ok() {
                        self.write_without_resp = !self.write_without_resp;
                        Ok(())
                    } else {
                        Err(format!("BLE write: {first_err}"))
                    }
                }
            }
        }
    }

    impl ElmTransport for BleNotifyTransport {
        fn transport_name(&self) -> &'static str {
            "ble"
        }

        fn send_command(&mut self, cmd: &str, timeout: Duration) -> Result<String, String> {
            // Drain any stale notifications non-blockingly before writing.
            let _ = self.conn.process(Duration::from_millis(0));
            self.rx_buf.lock().unwrap().clear();

            self.write_raw(format!("{cmd}\r").into_bytes())?;

            let deadline = Instant::now() + timeout;
            loop {
                let _ = self.conn.process(Duration::from_millis(15));
                {
                    let guard = self.rx_buf.lock().unwrap();
                    if guard.contains(&b'>') || guard.len() > 4096 {
                        let text = String::from_utf8_lossy(&guard)
                            .replace('\r', "\n")
                            .replace('>', "");
                        return Ok(text);
                    }
                }
                if Instant::now() >= deadline {
                    let guard = self.rx_buf.lock().unwrap();
                    if !guard.is_empty() {
                        let text = String::from_utf8_lossy(&guard)
                            .replace('\r', "\n")
                            .replace('>', "");
                        let trimmed = text.trim();
                        if !trimmed.is_empty() && trimmed != "SEARCHING..." {
                            return Ok(text);
                        }
                    }
                    return Err(format!("Timeout réponse {cmd}"));
                }
            }
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
                    let text = String::from_utf8_lossy(&buf)
                        .replace('\r', "\n")
                        .replace('>', "");
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && trimmed != "SEARCHING..." {
                        return Ok(text);
                    }
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
                        if buf.contains(&b'>') || buf.len() > 4096 {
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
        let mut fallback_notify: Option<String> = None;
        let mut fallback_write: Option<(String, bool)> = None;

        for (path, ifaces) in objs {
            let pstr = path.to_string();
            if !pstr.starts_with(dev_path) {
                continue;
            }
            if let Some(ch) = ifaces.get("org.bluez.GattCharacteristic1") {
                if let Some(uuid) = prop_str(ch, "UUID") {
                    let u = uuid.to_ascii_lowercase();
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
                    // Ignore standard GAP/GATT service changed char (2a05).
                    let is_std = u.starts_with("00002a0");
                    if !is_std {
                        if fallback_notify.is_none()
                            && flags.iter().any(|f| f == "notify" || f == "indicate")
                        {
                            fallback_notify = Some(pstr.clone());
                        }
                        if fallback_write.is_none()
                            && flags
                                .iter()
                                .any(|f| f == "write-without-response" || f == "write")
                        {
                            let no_resp = flags.iter().any(|f| f == "write-without-response");
                            fallback_write = Some((pstr.clone(), no_resp));
                        }
                    }
                    by_uuid.insert(u, (pstr, flags));
                }
            }
        }
        for &(notify_u, write_u) in BLE_PAIRS {
            if let (Some((np, _)), Some((wp, wflags))) = (by_uuid.get(notify_u), by_uuid.get(write_u)) {
                let no_resp = wflags.iter().any(|f| f == "write-without-response");
                return Ok(Some((np.clone(), wp.clone(), no_resp)));
            }
        }
        if let (Some(np), Some((wp, no_resp))) = (fallback_notify, fallback_write) {
            return Ok(Some((np, wp, no_resp)));
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

    pub fn connect_elm(
        svc: &Arc<ObdService>,
        mac: &str,
    ) -> Result<(Box<dyn ElmTransport>, String), String> {
        let conn = Connection::new_system().map_err(|e| format!("D-Bus: {e}"))?;
        let dev_path = format!("/org/bluez/hci0/dev_{}", mac.replace(':', "_"));
        let dev = conn.with_proxy("org.bluez", &dev_path, Duration::from_secs(12));

        push_log(svc, format!("BT: connexion à {mac}..."));
        let _ = dev.set("org.bluez.Device1", "Trusted", true);

        let mut connect_res: Result<(), dbus::Error> =
            dev.method_call("org.bluez.Device1", "Connect", ());
        if let Err(ref e1) = connect_res {
            push_log(svc, format!("BT 1er essai: {e1} -> scan LE..."));
            run_discovery(&conn, "le", Duration::from_secs(4));
            let _ = dev.set("org.bluez.Device1", "Trusted", true);
            connect_res = dev.method_call("org.bluez.Device1", "Connect", ());
            if let Err(ref e2) = connect_res {
                push_log(svc, format!("BT 2e essai: {e2}"));
                std::thread::sleep(Duration::from_millis(600));
                connect_res = dev.method_call("org.bluez.Device1", "Connect", ());
            }
        }
        connect_res.map_err(|e| format!("Bluetooth connect ({mac}): {e}"))?;

        // Wait up to 8 s for GATT ServicesResolved (BLE).
        let mut dev_name = mac.to_string();
        let mut resolved = false;
        for _ in 0..40 {
            let root = conn.with_proxy("org.bluez", "/", Duration::from_secs(3));
            if let Ok(objs) = root.get_managed_objects() {
                if let Some(ifaces) = objs.get(&dbus::Path::new(&dev_path).unwrap()) {
                    if let Some(props) = ifaces.get("org.bluez.Device1") {
                        if let Some(n) = prop_str(props, "Name").or_else(|| prop_str(props, "Alias")) {
                            dev_name = n;
                        }
                        if prop_bool(props, "ServicesResolved") {
                            resolved = true;
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        push_log(svc, format!("BT lié ({dev_name}), GATT resolved={resolved}"));

        if let Some((notify_path, write_path, write_without_resp)) = find_ble_chars(&conn, &dev_path)? {
            push_log(
                svc,
                format!("BLE GATT trouvé (cmd_no_resp={write_without_resp}) -> StartNotify"),
            );
            let rx_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let rx_cb = Arc::clone(&rx_buf);
            let rule = MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged")
                .with_path(notify_path.clone());
            let token = conn
                .add_match(rule, move |sig: PropertiesPropertiesChanged, _, _| {
                    if sig.interface_name == "org.bluez.GattCharacteristic1" {
                        if let Some(val) = sig.changed_properties.get("Value") {
                            if let Some(iter) = val.0.as_iter() {
                                let mut guard = rx_cb.lock().unwrap();
                                for item in iter {
                                    if let Some(b) = item.as_u64() {
                                        guard.push(b as u8);
                                    }
                                }
                            }
                        }
                    }
                    true
                })
                .map_err(|e| format!("BLE match: {e}"))?;

            let nproxy = conn.with_proxy("org.bluez", &notify_path, Duration::from_secs(5));
            let start_res: Result<(), _> =
                nproxy.method_call("org.bluez.GattCharacteristic1", "StartNotify", ());
            start_res.map_err(|e| format!("BLE StartNotify: {e}"))?;

            return Ok((
                Box::new(BleNotifyTransport {
                    conn,
                    notify_path,
                    write_path,
                    write_without_resp,
                    rx_buf,
                    _match_token: token,
                }),
                dev_name,
            ));
        }

        // Fallback to Classic SPP RFCOMM (e.g., "Android-Vlink").
        push_log(svc, "Pas de GATT BLE -> tentative RFCOMM SPP...");
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
            estimate_diesel_fuel_lph(Some(4.8), Some(15.0), Some(2.0), Some(30.0), None, None, None),
            Some(4.8)
        );
        // Wideband Lambda (PID 0x34) + MAF (PID 0x10) exact Fiat MultiJet calculation:
        // MAF 25 g/s, lambda 2.5 -> AFR 36.25 -> 0.6897 g/s -> 2.97 L/h.
        let exact = estimate_diesel_fuel_lph(None, Some(25.0), Some(2.5), Some(40.0), None, None, None).unwrap();
        assert!(exact > 2.8 && exact < 3.2, "unexpected lambda diesel rate: {exact}");
        // Overrun fuel cut (lambda > 7.5) -> 0.0 L/h.
        assert_eq!(
            estimate_diesel_fuel_lph(None, Some(15.0), Some(9.0), Some(5.0), None, None, None),
            Some(0.0)
        );
        // MAF + Load fallback when Lambda is absent.
        let est = estimate_diesel_fuel_lph(None, Some(20.0), None, Some(35.0), None, None, None).unwrap();
        assert!(est > 1.5 && est < 3.0, "unexpected diesel estimate: {est}");
    }

    #[test]
    fn recognizes_vgate_obd_names() {
        assert!(is_likely_obd_name("IOS-Vlink"));
        assert!(is_likely_obd_name("Android-Vlink"));
        assert!(is_likely_obd_name("Vgate iCar Pro"));
        assert!(!is_likely_obd_name("iPhone de Aymene"));
    }

    #[test]
    fn decodes_gear_power_vin_and_fiat_uds_dpf() {
        assert_eq!(estimate_gear(0.0, 820.0), "N");
        assert_eq!(estimate_gear(30.0, 2000.0), "2");
        assert_eq!(estimate_gear(90.0, 2000.0), "5");
        assert_eq!(estimate_gear(130.0, 2400.0), "6");

        let (hp, nm) = estimate_diesel_power_torque(Some(10.0), Some(2000.0));
        assert!(hp.unwrap() > 40.0 && hp.unwrap() < 60.0);
        assert!(nm.unwrap() > 140.0 && nm.unwrap() < 200.0);

        let uds = parse_uds_22("62 18 E4 0F A0\r\n>", 0x18, 0xE4).unwrap();
        assert_eq!(uds, vec![0x0F, 0xA0]);

        let vin = parse_vin("014\r\n0: 49 02 01 5A 46 41\r\n1: 33 35 36 30 30 30 30\r\n2: 31 32 33 34 35 36 37\r\n>").unwrap();
        assert_eq!(vin, "ZFA35600001234567");

        // UDS Service 19 02 (59 02 FF <P1206 FTB=00 status=09 confirmed> <P0401 FTB=11 status=04 pending>)
        let uds_dtcs = parse_uds_1902("00B\r\n0: 59 02 FF 12 06 00\r\n1: 09 04 01 11 04 00 00\r\n>");
        assert_eq!(uds_dtcs.len(), 2);
        assert_eq!(uds_dtcs[0].code, "P1206");
        assert!(!uds_dtcs[0].pending);
        assert_eq!(uds_dtcs[1].code, "P0401");
        assert!(uds_dtcs[1].pending);
    }
}
