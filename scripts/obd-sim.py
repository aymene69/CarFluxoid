#!/usr/bin/env python3
"""
CarFluxoid Interactive Virtual OBD-II Simulator (Fiat Tipo MultiJet)
--------------------------------------------------------------------
Zero external dependencies (works with any Python 3.9+ — no tkinter/pygame needed!).
Launches:
  1. The real native 800x480 CarFluxoid Uconnect Slint preview window (`dashboard-ui --preview`)
  2. A sleek interactive Web Driving Console at http://127.0.0.1:8088 (opens in your browser)
  3. A CarFluxoid-compatible HTTP API on http://0.0.0.0:8088/api/obd/live (testable from iPhone!)
"""

import http.server
import json
import math
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import webbrowser
from pathlib import Path

PREVIEW_DIR = Path(tempfile.gettempdir()) / "carfluxoid-preview-settings"
OBD_JSON = PREVIEW_DIR / "obd.json"
DISPLAY_FILE = PREVIEW_DIR / "video.sock.display"
HTTP_PORT = 8088

GEAR_RATIOS = {
    1: 8.5,   # km/h per 1000 rpm
    2: 15.5,
    3: 24.0,
    4: 33.5,
    5: 43.5,
    6: 54.0,
}

CONSOLE_HTML = r"""<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>CarFluxoid — Simulateur OBD-II Fiat Tipo MultiJet</title>
<style>
  :root {
    --bg: #080d16; --surface: #121c2e; --alt: #172338; --border: rgba(255,255,255,0.09);
    --accent: #14d4b4; --blue: #38bdf8; --amber: #f59e0b; --red: #f87171; --muted: #8ea2be;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Display", sans-serif;
    background: var(--bg); color: #f4f7fc; padding: 18px; max-width: 620px; margin-inline: auto;
  }
  h1 { font-size: 20px; margin: 0 0 4px; color: var(--accent); }
  .sub { font-size: 12px; color: var(--muted); margin-bottom: 14px; }
  .card {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: 16px; padding: 14px 16px; margin-bottom: 12px;
  }
  .hud { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; text-align: center; }
  .hud .k { font-size: 10px; text-transform: uppercase; color: var(--muted); letter-spacing: .6px; }
  .hud .v { font-size: 22px; font-weight: 800; font-variant-numeric: tabular-nums; margin-top: 2px; }
  .v.accent { color: var(--accent); } .v.blue { color: var(--blue); } .v.amber { color: var(--amber); }
  .row { display: flex; justify-content: space-between; align-items: center; margin-bottom: 4px; font-size: 12px; color: var(--muted); }
  .row strong { color: #fff; font-variant-numeric: tabular-nums; }
  input[type=range] { width: 100%; accent-color: var(--accent); height: 26px; cursor: pointer; }
  .pedals { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-bottom: 10px; }
  .pedal {
    padding: 16px; border-radius: 14px; border: 1px solid var(--border); font-weight: 800; font-size: 15px;
    cursor: pointer; user-select: none; -webkit-user-select: none; transition: transform .05s;
  }
  .pedal:active, .pedal.on { transform: scale(0.97); }
  .gas { background: rgba(20,212,180,0.16); color: var(--accent); border-color: rgba(20,212,180,0.45); }
  .gas.on { background: var(--accent); color: #041c1a; }
  .brk { background: rgba(248,113,113,0.16); color: var(--red); border-color: rgba(248,113,113,0.45); }
  .brk.on { background: var(--red); color: #1c0404; }
  .gears { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
  .gbtn {
    flex: 1; min-width: 38px; padding: 9px 0; border-radius: 10px; border: 1px solid var(--border);
    background: var(--alt); color: #fff; font-weight: 700; cursor: pointer;
  }
  .gbtn.active { background: var(--accent); color: #041c1a; border-color: var(--accent); }
  .toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .tbtn {
    padding: 10px 12px; border-radius: 12px; border: 1px solid var(--border);
    background: var(--alt); color: var(--muted); font-weight: 700; font-size: 12px; cursor: pointer;
  }
  .tbtn.on { background: rgba(20,212,180,0.2); color: var(--accent); border-color: var(--accent); }
  .tbtn.warn.on { background: rgba(245,158,11,0.24); color: var(--amber); border-color: var(--amber); }
  .hint { font-size: 11px; color: #647896; text-align: center; margin-top: 8px; }
</style>
</head>
<body>
  <h1>CarFluxoid · Simulateur Fiat Tipo MultiJet</h1>
  <div class="sub">Pilote la fenêtre Uconnect 800×480 en direct · API iPhone : <code id="apiUrl"></code></div>

  <div class="card">
    <div class="hud">
      <div><div class="k">Vitesse</div><div class="v blue" id="spd">0</div></div>
      <div><div class="k">Régime</div><div class="v" id="rpm">820</div></div>
      <div><div class="k">Rapport</div><div class="v accent" id="gear">N</div></div>
      <div><div class="k">Turbo</div><div class="v amber" id="boost">+0.00</div></div>
    </div>
    <div class="hint" id="subHud">0 ch · 0 Nm · λ 5.00 · Conso —</div>
  </div>

  <div class="card">
    <div class="pedals">
      <button class="pedal gas" id="btnGas">⚡ ACCÉLÉRER (↑ / Z)</button>
      <button class="pedal brk" id="btnBrk">🛑 FREINER (↓ / S)</button>
    </div>
    <div class="row"><span>Accélérateur maintenu</span><strong id="vThr">0 %</strong></div>
    <input type="range" id="sThr" min="0" max="100" value="0">
    <div class="row"><span>Niveau carburant PID 012F (+20% = test auto-plein)</span><strong id="vFuel">64 %</strong></div>
    <input type="range" id="sFuel" min="0" max="100" value="64">
    <div class="row"><span>Température eau moteur</span><strong id="vCool">88 °C</strong></div>
    <input type="range" id="sCool" min="20" max="110" value="88">
  </div>

  <div class="card">
    <div class="row"><span>Boîte de vitesses (clavier E / Q ou 0–6)</span><strong id="gMode">Auto</strong></div>
    <div class="gears" id="gearRow"></div>
  </div>

  <div class="card">
    <div class="toggles">
      <button class="tbtn on" id="tAuto">Boîte Auto (A)</button>
      <button class="tbtn" id="tDemo">Pilote Démo (D)</button>
      <button class="tbtn warn" id="tFap">Régén. FAP 620°C (R)</button>
      <button class="tbtn on" id="tMarg">Marges Uconnect 32px</button>
    </div>
  </div>

  <div class="hint">Clavier actif sur cette page : ↑/Z Accélérer · ↓/S Freiner · E/Q Rapports · A Auto · R FAP · D Démo</div>

<script>
document.getElementById("apiUrl").textContent = location.origin;
const ctrl = { throttle: 0, brake: 0, auto_gearbox: true, autopilot: false, dpf_regen: false, margins_32: true, gear: 0, fuel_pct: 64, coolant_c: 88 };
const keys = new Set();
let mouseGas = false, mouseBrk = false;

const gearRow = document.getElementById("gearRow");
for (let g = 0; g <= 6; g++) {
  const b = document.createElement("button");
  b.className = "gbtn"; b.textContent = g === 0 ? "N" : "V" + g; b.dataset.g = g;
  b.onclick = () => { ctrl.auto_gearbox = false; ctrl.gear = g; pushCtrl({ auto_gearbox: false, gear: g }); };
  gearRow.appendChild(b);
}

function pushCtrl(patch) {
  Object.assign(ctrl, patch);
  fetch("/sim/control", { method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(patch) }).catch(()=>{});
  syncButtons();
}

function syncButtons() {
  document.getElementById("tAuto").classList.toggle("on", ctrl.auto_gearbox);
  document.getElementById("tDemo").classList.toggle("on", ctrl.autopilot);
  document.getElementById("tFap").classList.toggle("on", ctrl.dpf_regen);
  document.getElementById("tMarg").classList.toggle("on", ctrl.margins_32);
  document.getElementById("gMode").textContent = ctrl.auto_gearbox ? "Automatique" : "Manuelle";
}

document.getElementById("sThr").oninput = e => { ctrl.autopilot = false; document.getElementById("vThr").textContent = e.target.value + " %"; pushCtrl({ throttle: +e.target.value, autopilot: false }); };
document.getElementById("sFuel").oninput = e => { document.getElementById("vFuel").textContent = e.target.value + " %"; pushCtrl({ fuel_pct: +e.target.value }); };
document.getElementById("sCool").oninput = e => { document.getElementById("vCool").textContent = e.target.value + " °C"; pushCtrl({ coolant_c: +e.target.value }); };

document.getElementById("tAuto").onclick = () => pushCtrl({ auto_gearbox: !ctrl.auto_gearbox });
document.getElementById("tDemo").onclick = () => pushCtrl({ autopilot: !ctrl.autopilot });
document.getElementById("tFap").onclick = () => pushCtrl({ dpf_regen: !ctrl.dpf_regen });
document.getElementById("tMarg").onclick = () => pushCtrl({ margins_32: !ctrl.margins_32 });

const btnGas = document.getElementById("btnGas"), btnBrk = document.getElementById("btnBrk");
btnGas.onpointerdown = () => { mouseGas = true; btnGas.classList.add("on"); };
btnGas.onpointerup = btnGas.onpointerleave = () => { mouseGas = false; btnGas.classList.remove("on"); };
btnBrk.onpointerdown = () => { mouseBrk = true; btnBrk.classList.add("on"); };
btnBrk.onpointerup = btnBrk.onpointerleave = () => { mouseBrk = false; btnBrk.classList.remove("on"); };

window.addEventListener("keydown", e => {
  const k = e.key.toLowerCase();
  if (["arrowup","arrowdown"," "].includes(k)) e.preventDefault();
  if (!keys.has(k)) {
    keys.add(k);
    if (k === "e") pushCtrl({ auto_gearbox: false, gear: Math.min(6, ctrl.gear + 1) });
    else if (k === "q") pushCtrl({ auto_gearbox: false, gear: Math.max(0, ctrl.gear - 1) });
    else if ("0123456".includes(k)) pushCtrl({ auto_gearbox: false, gear: +k });
    else if (k === "a") pushCtrl({ auto_gearbox: !ctrl.auto_gearbox });
    else if (k === "r") pushCtrl({ dpf_regen: !ctrl.dpf_regen });
    else if (k === "d") pushCtrl({ autopilot: !ctrl.autopilot });
  }
});
window.addEventListener("keyup", e => keys.delete(e.key.toLowerCase()));

// Smooth keyboard/mouse pedal loop (20 Hz)
setInterval(() => {
  const gasOn = mouseGas || keys.has("arrowup") || keys.has("z") || keys.has("w");
  const brkOn = mouseBrk || keys.has("arrowdown") || keys.has("s");
  btnGas.classList.toggle("on", gasOn);
  btnBrk.classList.toggle("on", brkOn);
  let patch = {};
  if (gasOn) {
    ctrl.autopilot = false;
    ctrl.throttle = Math.min(100, ctrl.throttle + 8);
    document.getElementById("sThr").value = Math.round(ctrl.throttle);
    document.getElementById("vThr").textContent = Math.round(ctrl.throttle) + " %";
    patch.throttle = ctrl.throttle; patch.autopilot = false;
  } else if (keys.size === 0 && !mouseGas && !ctrl.autopilot && ctrl.throttle > 0 && document.activeElement !== document.getElementById("sThr")) {
    ctrl.throttle = Math.max(0, ctrl.throttle - 10);
    document.getElementById("sThr").value = Math.round(ctrl.throttle);
    document.getElementById("vThr").textContent = Math.round(ctrl.throttle) + " %";
    patch.throttle = ctrl.throttle;
  }
  if (brkOn) {
    ctrl.brake = Math.min(100, ctrl.brake + 18);
    patch.brake = ctrl.brake;
  } else if (ctrl.brake > 0) {
    ctrl.brake = Math.max(0, ctrl.brake - 24);
    patch.brake = ctrl.brake;
  }
  if (Object.keys(patch).length) pushCtrl(patch);
}, 50);

// Live HUD refresh from simulator (8 Hz)
setInterval(async () => {
  try {
    const r = await fetch("/api/obd/live");
    const d = await r.json();
    document.getElementById("spd").textContent = Math.round(d.speed_kmh || 0) + " km/h";
    document.getElementById("rpm").textContent = Math.round(d.rpm || 0);
    document.getElementById("gear").textContent = d.gear || "N";
    document.getElementById("boost").textContent = (d.boost_bar >= 0 ? "+" : "") + (d.boost_bar || 0).toFixed(2);
    const cons = d.fuel_l_per_100km != null ? d.fuel_l_per_100km.toFixed(1) + " L/100" : (d.fuel_rate_lph || 0).toFixed(1) + " L/h";
    document.getElementById("subHud").textContent = `${d.power_hp || 0} ch · ${d.torque_nm || 0} Nm · λ ${(d.lambda || 0).toFixed(2)} · Conso ${cons} · FAP ${d.dpf_temp_c || 0}°C`;
    ctrl.gear = d.gear === "N" ? 0 : parseInt(d.gear || "0", 10);
    for (const b of gearRow.children) b.classList.toggle("active", +b.dataset.g === ctrl.gear);
  } catch (_) {}
}, 125);
syncButtons();
</script>
</body>
</html>
"""


class MultiJetSim:
    def __init__(self):
        self.lock = threading.Lock()
        self.throttle_cmd = 0.0
        self.brake_cmd = 0.0
        self.auto_gearbox = True
        self.autopilot = False
        self.dpf_regen = False
        self.margins_32 = True

        self.speed_kmh = 0.0
        self.rpm = 820.0
        self.gear = 0
        self.boost_bar = -0.04
        self.coolant_c = 88.0
        self.fuel_pct = 64.0
        self.dpf_temp_c = 285.0
        self.dpf_soot_pct = 42.0
        self.battery_v = 14.3
        self.dist_since_clear = 14820
        self.start_time = time.time()
        self.trip_dist_km = 0.0
        self.lat = 45.7640
        self.lon = 4.8357
        self.heading = 0.6

    def apply_control(self, data: dict):
        with self.lock:
            if "throttle" in data:
                self.throttle_cmd = max(0.0, min(100.0, float(data["throttle"])))
            if "brake" in data:
                self.brake_cmd = max(0.0, min(100.0, float(data["brake"])))
            if "auto_gearbox" in data:
                self.auto_gearbox = bool(data["auto_gearbox"])
            if "autopilot" in data:
                self.autopilot = bool(data["autopilot"])
            if "dpf_regen" in data:
                self.dpf_regen = bool(data["dpf_regen"])
            if "gear" in data:
                self.gear = max(0, min(6, int(data["gear"])))
            if "fuel_pct" in data:
                self.fuel_pct = max(0.0, min(100.0, float(data["fuel_pct"])))
            if "coolant_c" in data:
                self.coolant_c = max(20.0, min(120.0, float(data["coolant_c"])))
            if "margins_32" in data:
                self.margins_32 = bool(data["margins_32"])
                write_atomic(DISPLAY_FILE, "800 480 0 32\n" if self.margins_32 else "800 480 0 0\n")

    def step(self, dt: float):
        with self.lock:
            now = time.time()
            if self.autopilot:
                phase = (now - self.start_time) * 0.35
                self.throttle_cmd = max(0.0, min(100.0, 42.0 + 42.0 * math.sin(phase)))
                self.brake_cmd = max(0.0, -35.0 * math.sin(phase))

            thr = self.throttle_cmd / 100.0
            brk = self.brake_cmd / 100.0

            if self.auto_gearbox:
                if self.speed_kmh < 1.5 and thr < 0.05:
                    self.gear = 0
                else:
                    if self.gear == 0:
                        self.gear = 1
                    ratio = GEAR_RATIOS[self.gear]
                    est_rpm = self.speed_kmh / ratio * 1000.0
                    up_rpm = 2300 + thr * 1400
                    down_rpm = 1350
                    if est_rpm > up_rpm and self.gear < 6:
                        self.gear += 1
                    elif est_rpm < down_rpm and self.gear > 1:
                        self.gear -= 1

            gear_mult = {0: 0.0, 1: 3.1, 2: 2.0, 3: 1.35, 4: 0.95, 5: 0.72, 6: 0.55}[self.gear]
            accel = thr * 5.8 * gear_mult - brk * 24.0 - 0.12 - (self.speed_kmh ** 2) * 0.00018
            if self.gear == 0:
                accel = -brk * 24.0 - 0.25
            self.speed_kmh = max(0.0, min(205.0, self.speed_kmh + accel * dt * 3.6))

            if self.gear == 0:
                target_rpm = 820.0 + thr * 3400.0
            else:
                wheel_rpm = (self.speed_kmh / GEAR_RATIOS[self.gear]) * 1000.0
                target_rpm = max(820.0, min(4850.0, wheel_rpm))
            self.rpm += (target_rpm - self.rpm) * min(1.0, dt * 8.0)

            load = max(8.0, min(100.0, 12.0 + thr * 88.0 * (min(self.rpm, 2600.0) / 2200.0)))
            target_boost = -0.05 + (load / 100.0) * 1.55 * min(1.0, max(0.15, (self.rpm - 900.0) / 1400.0))
            self.boost_bar += (target_boost - self.boost_bar) * min(1.0, dt * 3.8)

            map_kpa = 101.3 + self.boost_bar * 100.0
            maf_gps = max(6.5, (map_kpa * 1.598 / (8.3145 * 300.0)) * (self.rpm / 120.0) * 0.88 * 28.97)
            if thr < 0.02 and self.speed_kmh > 18.0 and self.gear > 0:
                lam = 8.80
                lph = 0.0
            else:
                lam = max(1.18, min(5.8, 5.2 - (load / 100.0) * 3.9))
                afr = 14.5 * lam
                lph = (maf_gps / afr) * 3.6 / 0.835

            l100 = round(min(45.0, (lph / self.speed_kmh) * 100.0), 1) if self.speed_kmh >= 5.0 else None
            kw = min(125.0, max(0.0, lph * 3.58))
            power_hp = round(kw * 1.3596)
            torque_nm = round(min(360.0, (kw * 9549.3) / max(self.rpm, 800.0)))

            egr_pct = round(max(0.0, min(85.0, 58.0 - load * 0.62)), 1)
            egr_err = round(math.sin(now * 1.3) * 1.8, 1)
            rail_bar = round(310.0 + (load / 100.0) * 1280.0)

            target_dpf = 625.0 if self.dpf_regen else (210.0 + load * 2.1)
            self.dpf_temp_c += (target_dpf - self.dpf_temp_c) * min(1.0, dt * 1.6)
            if self.dpf_regen and self.dpf_soot_pct > 6.0:
                self.dpf_soot_pct = max(6.0, self.dpf_soot_pct - dt * 0.4)

            step_km = self.speed_kmh * (dt / 3600.0)
            self.trip_dist_km += step_km
            self.fuel_pct = max(0.0, self.fuel_pct - (lph * (dt / 3600.0) / 50.0) * 100.0)
            if self.speed_kmh > 1.0:
                self.heading += 0.03 * math.sin(now * 0.25) * dt
                self.lat += (step_km / 111.0) * math.cos(self.heading)
                self.lon += (step_km / (111.0 * math.cos(math.radians(self.lat)))) * math.sin(self.heading)

            return {
                "connected": True,
                "state": "connected",
                "adapter_mac": "00:1D:A5:04:51:44",
                "adapter_name": "IOS-Vlink (Simulateur)",
                "transport": "ble",
                "elm_version": "ELM327 v2.3 (Vgate Sim)",
                "protocol": "AUTO, ISO 15765-4 (CAN 11/500)",
                "supported_pids": [
                    "0101", "0104", "0105", "010B", "010C", "010D", "010F", "0110",
                    "011F", "0123", "012C", "012D", "012F", "0130", "0131", "0133",
                    "0134", "0142", "0149",
                ],
                "rpm": round(self.rpm),
                "speed_kmh": round(self.speed_kmh, 1),
                "coolant_c": round(self.coolant_c),
                "intake_c": 26.0,
                "load_pct": round(load, 1),
                "throttle_pct": round(self.throttle_cmd, 1),
                "map_kpa": round(map_kpa, 1),
                "baro_kpa": 101.3,
                "boost_bar": round(self.boost_bar, 2),
                "maf_gps": round(maf_gps, 1),
                "rail_bar": rail_bar,
                "fuel_level_pct": round(self.fuel_pct, 1),
                "fuel_rate_lph": round(lph, 2),
                "fuel_l_per_100km": l100,
                "battery_v": round(self.battery_v, 2),
                "egr_pct": egr_pct,
                "egr_error_pct": egr_err,
                "lambda": round(lam, 2),
                "pedal_pct": round(self.throttle_cmd, 1),
                "dpf_temp_c": round(self.dpf_temp_c),
                "dpf_soot_pct": round(self.dpf_soot_pct, 1),
                "dpf_regen_active": self.dpf_temp_c >= 480.0,
                "gear": "N" if self.gear == 0 else str(self.gear),
                "power_hp": power_hp,
                "torque_nm": torque_nm,
                "runtime_s": int(now - self.start_time),
                "dist_since_clear_km": self.dist_since_clear + int(self.trip_dist_km),
                "warmups_since_clear": 255,
                "vin": "ZFA3560000SIM1234",
                "gps": {
                    "lat": round(self.lat, 6),
                    "lon": round(self.lon, 6),
                    "alt_m": 175.0,
                    "speed_kmh": round(self.speed_kmh, 1),
                    "bearing": round(math.degrees(self.heading) % 360, 1),
                    "ts": int(now),
                },
                "dtcs": [],
                "debug_log": [
                    "Simulateur Fiat Tipo 1.6 MultiJet actif",
                    f"RPM={round(self.rpm)} V={round(self.speed_kmh)}km/h Boost={self.boost_bar:+.2f}bar λ={lam:.2f}",
                ],
                "updated_at": int(now),
            }


def write_atomic(path: Path, text: str):
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)


def local_ip() -> str:
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except Exception:
        return "127.0.0.1"


def main():
    PREVIEW_DIR.mkdir(parents=True, exist_ok=True)
    write_atomic(DISPLAY_FILE, "800 480 0 32\n")

    sim = MultiJetSim()
    latest = [sim.step(0.05)]
    write_atomic(OBD_JSON, json.dumps(latest[0]))
    stop_event = threading.Event()

    def physics_loop():
        while not stop_event.is_set():
            snap = sim.step(0.08)
            latest[0] = snap
            write_atomic(OBD_JSON, json.dumps(snap))
            time.sleep(0.08)

    threading.Thread(target=physics_loop, daemon=True).start()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def _send(self, code, ctype, data: bytes):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path in ("/", "/index.html"):
                self._send(200, "text/html; charset=utf-8", CONSOLE_HTML.encode("utf-8"))
            elif self.path == "/api/obd/live":
                self._send(200, "application/json; charset=utf-8", json.dumps(latest[0]).encode("utf-8"))
            elif self.path == "/diag":
                diag = {
                    "version": "1.3.1-sim", "build": "mac-sim", "slot": "A",
                    "temp_c": "38.2 °C", "uptime_s": 120,
                    "display": "800x480 (marges 0x32)" if sim.margins_32 else "800x480",
                    "night_mode": "nuit", "gps": "Simulateur actif",
                    "gps_carplay": "actif", "guidance": "inactif",
                    "carplay_active": False, "airplay_active": False,
                }
                self._send(200, "application/json; charset=utf-8", json.dumps(diag).encode("utf-8"))
            elif self.path in ("/api/trips", "/api/obd/devices"):
                self._send(200, "application/json; charset=utf-8", b"[]")
            else:
                self._send(404, "text/plain", b"Not found")

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length) if length > 0 else b"{}"
            if self.path == "/sim/control":
                try:
                    sim.apply_control(json.loads(body.decode("utf-8")))
                except Exception:
                    pass
                self._send(200, "application/json", b'{"ok":true}')
            elif self.path == "/api/time":
                self._send(200, "application/json", json.dumps({"unix": int(time.time())}).encode("utf-8"))
            else:
                self._send(200, "application/json", json.dumps(latest[0]).encode("utf-8"))

    class ReuseServer(socketserver.ThreadingTCPServer):
        allow_reuse_address = True

    srv = ReuseServer(("0.0.0.0", HTTP_PORT), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    ip = local_ip()
    print(f"Console de pilotage OBD : http://127.0.0.1:{HTTP_PORT}")
    print(f"API iPhone (optionnel)  : http://{ip}:{HTTP_PORT}")
    if "--no-browser" not in sys.argv:
        webbrowser.open(f"http://127.0.0.1:{HTTP_PORT}")

    repo_root = Path(__file__).resolve().parent.parent
    ui_proc = None
    try:
        if "--no-ui" not in sys.argv:
            cmd = [
                "cargo", "run",
                "--manifest-path", str(repo_root / "dashboard-ui" / "Cargo.toml"),
                "--features", "preview",
                "--", "--preview",
            ]
            print("Lancement de la fenêtre CarFluxoid Uconnect 800x480 (Slint)… (Ctrl+C pour quitter)")
            ui_proc = subprocess.Popen(cmd, cwd=str(repo_root))
            ui_proc.wait()
        else:
            print("Mode sans fenêtre UI (--no-ui). Ctrl+C pour quitter.")
            while True:
                time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        stop_event.set()
        srv.shutdown()
        if ui_proc and ui_proc.poll() is None:
            ui_proc.terminate()


if __name__ == "__main__":
    main()
