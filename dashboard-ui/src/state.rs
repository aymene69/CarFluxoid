//! Dashboard settings persistence, live vehicle/Pi telemetry, and CarPlay lifecycle control.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
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

pub struct DashboardController {
    pub paths: Paths,
    pub active_page: String,
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
            dry_run,
        }
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
