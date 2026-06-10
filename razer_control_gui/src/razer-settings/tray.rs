use std::fs;
use std::sync::{Arc, Mutex};
use serde_json;

#[derive(Clone)]
pub struct SensorState {
    pub cpu_temp: Option<f64>,
    pub igpu_temp: Option<f64>,
    pub dgpu_temp: Option<f64>,
    pub fan_speed: Option<i32>,
    pub on_ac: Option<bool>,
    pub battery_pct: Option<u8>,
    pub battery_status: Option<String>,
    pub battery_power: Option<f64>,
    pub system_power: Option<f64>,
    pub cpu_util: Option<u32>,
    pub igpu_power: Option<f64>,
    pub igpu_util: Option<u32>,
    pub dgpu_power: Option<f64>,
    pub dgpu_util: Option<u32>,
    pub fan_min: i32,
    pub fan_max: i32,
}

impl Default for SensorState {
    fn default() -> Self {
        SensorState {
            cpu_temp: None, igpu_temp: None, dgpu_temp: None,
            fan_speed: None, on_ac: None, battery_pct: None,
            battery_status: None, battery_power: None, system_power: None,
            cpu_util: None, igpu_power: None, igpu_util: None,
            dgpu_power: None, dgpu_util: None,
            fan_min: 3500, fan_max: 5000,
        }
    }
}

impl SensorState {
    /// Read all sensors directly from sysfs/nvidia-smi
    fn read_fresh() -> Self {
        SensorState {
            cpu_temp: read_cpu_temp(),
            igpu_temp: read_igpu_temp(),
            dgpu_temp: read_dgpu_temp(),
            fan_speed: None, // requires daemon, skip in tray
            on_ac: read_ac_power(),
            battery_pct: read_battery_pct(),
            battery_status: read_battery_status(),
            battery_power: read_battery_power(),
            system_power: read_system_power(),
            cpu_util: read_cpu_util(),
            igpu_power: read_igpu_power(),
            igpu_util: read_igpu_util(),
            dgpu_power: read_dgpu_power(),
            dgpu_util: read_dgpu_util(),
            fan_min: 3500,
            fan_max: 5000,
        }
    }

    fn has_data(&self) -> bool {
        self.cpu_temp.is_some()
            || self.igpu_temp.is_some()
            || self.dgpu_temp.is_some()
            || self.fan_speed.is_some()
            || self.on_ac.is_some()
            || self.system_power.is_some()
    }

    fn format_lines(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        // CPU: merge temp, power, util into one line
        {
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = self.cpu_temp { parts.push(format!("{:.0}\u{00B0}C", t)); }
            if let Some(w) = self.system_power { parts.push(format!("{:.1}W", w)); }
            if let Some(u) = self.cpu_util { parts.push(format!("{}%", u)); }
            if !parts.is_empty() {
                lines.push(format!("CPU: {}", parts.join(" \u{00B7} ")));
            }
        }

        // iGPU: merge temp, power, util
        {
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = self.igpu_temp { parts.push(format!("{:.0}\u{00B0}C", t)); }
            if let Some(w) = self.igpu_power { parts.push(format!("{:.1}W", w)); }
            if let Some(u) = self.igpu_util { parts.push(format!("{}%", u)); }
            if !parts.is_empty() {
                lines.push(format!("iGPU: {}", parts.join(" \u{00B7} ")));
            }
        }

        // dGPU: merge temp, power, util
        {
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = self.dgpu_temp { parts.push(format!("{:.0}\u{00B0}C", t)); }
            if let Some(w) = self.dgpu_power { parts.push(format!("{:.1}W", w)); }
            if let Some(u) = self.dgpu_util { parts.push(format!("{}%", u)); }
            if !parts.is_empty() {
                lines.push(format!("dGPU: {}", parts.join(" \u{00B7} ")));
            }
        }

        if let Some(rpm) = self.fan_speed {
            if rpm == 0 {
                lines.push("Fan: Auto".into());
            } else {
                lines.push(format!("Fan: {} RPM", rpm));
            }
        }

        match (self.on_ac, self.battery_pct) {
            (Some(true), Some(pct)) => {
                let mut text = format!("AC / {}%", pct);
                if let Some(ref status) = self.battery_status {
                    if let Some(w) = self.battery_power {
                        if status == "Charging" {
                            text = format!("AC / {}% +{:.1}W", pct, w);
                        }
                    }
                    if status == "Not charging" {
                        text = format!("AC / {}% (Limit)", pct);
                    }
                }
                lines.push(text);
            }
            (Some(true), None) => lines.push("AC Power".into()),
            (Some(false), Some(pct)) => {
                let mut text = format!("Battery {}%", pct);
                if let Some(w) = self.battery_power {
                    text = format!("Battery {}% \u{2212}{:.1}W", pct, w);
                }
                lines.push(text);
            }
            (Some(false), None) => lines.push("Battery".into()),
            _ => {}
        }

        if lines.is_empty() {
            "Razer Control".into()
        } else {
            lines.join("\n")
        }
    }
}

pub type SharedSensorState = Arc<Mutex<SensorState>>;

pub fn new_shared_state() -> SharedSensorState {
    Arc::new(Mutex::new(SensorState::default()))
}

fn get_fan_range_from_daemon() -> (i32, i32) {
    let name = crate::comms::try_bind()
        .ok()
        .and_then(|socket| crate::comms::send_to_daemon(
            crate::comms::DaemonCommand::GetDeviceName, socket,
        ))
        .and_then(|resp| match resp {
            crate::comms::DaemonResponse::GetDeviceName { name } => Some(name),
            _ => None,
        });

    let name = match name {
        Some(n) => n,
        None => return (3500, 5000),
    };

    let path = std::env::var("RAZER_DEVICE_FILE")
        .unwrap_or_else(|_| "/usr/share/razercontrol/laptops.json".into());
    let json = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return (3500, 5000),
    };
    let devices: Vec<serde_json::Value> = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return (3500, 5000),
    };

    for device in devices {
        if device["name"].as_str() == Some(&name) {
            if let Some(fan) = device["fan"].as_array() {
                let min = fan.first().and_then(|v| v.as_i64()).unwrap_or(3500) as i32;
                let max = fan.get(1).and_then(|v| v.as_i64()).unwrap_or(5000) as i32;
                return (min, max);
            }
        }
    }
    (3500, 5000)
}

pub fn start_background_polling(state: SharedSensorState) -> std::sync::mpsc::Receiver<SensorState> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Retry until daemon is ready (it may not be up at app launch)
        let (fan_min, fan_max) = loop {
            let result = get_fan_range_from_daemon();
            if result != (3500, 5000) {
                break result;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        };
        if let Ok(mut s) = state.lock() {
            s.fan_min = fan_min;
            s.fan_max = fan_max;
        }

        loop {
            let fresh = SensorState::read_fresh();
            let ac = fresh.on_ac.unwrap_or(true);
            let fan_speed = crate::comms::try_bind()
                .ok()
                .and_then(|socket| crate::comms::send_to_daemon(
                    crate::comms::DaemonCommand::GetFanSpeed { ac: if ac { 1 } else { 0 } },
                    socket,
                ))
                .and_then(|resp| match resp {
                    crate::comms::DaemonResponse::GetFanSpeed { rpm } => Some(rpm),
                    _ => None,
                });

            if let Ok(mut s) = state.lock() {
                let (fan_min, fan_max) = (s.fan_min, s.fan_max);
                *s = SensorState { fan_speed, fan_min, fan_max, ..fresh };
                let _ = sender.send(s.clone()); // non-blocking, ok to drop if receiver gone
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
    receiver
}

pub struct RazerTray {
    state: SharedSensorState,
}

impl RazerTray {
    pub fn new(state: SharedSensorState) -> Self {
        RazerTray { state }
    }
}

impl ksni::Tray for RazerTray {
    fn id(&self) -> String {
        "razer-settings".into()
    }

    fn title(&self) -> String {
        "Razer Control".into()
    }

    fn icon_name(&self) -> String {
        "com.github.encomjp.razercontrol-symbolic".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        // Try shared state first (has fan speed from daemon); fall back to direct reads
        let body = if let Ok(s) = self.state.lock() {
            if s.has_data() {
                s.format_lines()
            } else {
                drop(s);
                SensorState::read_fresh().format_lines()
            }
        } else {
            SensorState::read_fresh().format_lines()
        };

        ksni::ToolTip {
            title: "Razer Control".into(),
            description: body,
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let state = self.state.lock().ok().map(|s| s.clone()).unwrap_or_default();
        let current_rpm = state.fan_speed.unwrap_or(0);
        let on_ac = state.on_ac.unwrap_or(true);
        let fan_min = state.fan_min;
        let fan_max = state.fan_max;
        let range = fan_max - fan_min;

        // Round to nearest 100 to match slider marks in the app
        let pct_rpm = |p: f64| -> i32 {
            ((fan_min as f64 + range as f64 * p) / 100.0).round() as i32 * 100
        };
        let presets: [(i32, &str); 6] = [
            (0,               "Auto"),
            (fan_min,         "Min"),
            (pct_rpm(0.25),   "25%"),
            (pct_rpm(0.50),   "50%"),
            (pct_rpm(0.75),   "75%"),
            (fan_max,         "Max"),
        ];

        let selected = presets.iter().enumerate()
            .min_by_key(|(_, (rpm, _))| {
                if *rpm == 0 && current_rpm == 0 { 0 }
                else if *rpm == 0 { i32::MAX }
                else { (current_rpm - rpm).abs() }
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        let current_preset = presets.get(selected).map(|(_, l)| *l).unwrap_or("Auto");
        let fan_submenu_label = format!("Fan Speed  ·  {}", current_preset);

        // Status lines
        fn stat_line(name: &str, temp: Option<f64>, util: Option<u32>) -> Option<String> {
            let right = match (temp, util) {
                (Some(t), Some(u)) => format!("{:.0}°C · {}%", t, u),
                (Some(t), None)    => format!("{:.0}°C", t),
                _ => return None,
            };
            Some(format!("{}\t{}", name, right))
        }
        let cpu_line  = stat_line("CPU",  state.cpu_temp,  state.cpu_util);
        let igpu_line = stat_line("iGPU", state.igpu_temp, state.igpu_util);
        let dgpu_line = stat_line("dGPU", state.dgpu_temp, state.dgpu_util);
        let bat_line = match (state.battery_pct, state.on_ac, state.battery_status.as_deref(), state.battery_power) {
            (Some(pct), Some(true), Some("Charging"), Some(w)) =>
                Some(format!("Battery  {}%  ·  AC +{:.0}W", pct, w)),
            (Some(pct), Some(true), _, _) =>
                Some(format!("Battery  {}%  ·  AC", pct)),
            (Some(pct), Some(false), _, Some(w)) =>
                Some(format!("Battery  {}%  ·  −{:.0}W", pct, w)),
            (Some(pct), Some(false), _, _) =>
                Some(format!("Battery  {}%", pct)),
            _ => None,
        };

        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            // Primary action — first
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: "Open Razer Control".into(),
                activate: Box::new(|_| {
                    let _ = std::process::Command::new("gdbus")
                        .args(["call", "--session",
                            "--dest", "com.encomjp.razer-settings",
                            "--object-path", "/com/encomjp/razer_settings",
                            "--method", "org.gtk.Application.Activate", "[]"])
                        .spawn();
                }),
                ..Default::default()
            }),
            ksni::MenuItem::Separator,
            // Fan control submenu
            ksni::MenuItem::SubMenu(ksni::menu::SubMenu {
                label: fan_submenu_label,
                submenu: presets.iter().enumerate().map(|(i, (rpm, label))| {
                    let rpm = *rpm;
                    let display = if rpm == 0 { label.to_string() } else { format!("{} ({} RPM)", label, rpm) };
                    ksni::MenuItem::Checkmark(ksni::menu::CheckmarkItem {
                        label: display,
                        checked: i == selected,
                        activate: Box::new(move |tray: &mut RazerTray| {
                            let ac = if on_ac { 1 } else { 0 };
                            let _ = crate::comms::try_bind()
                                .ok()
                                .and_then(|socket| crate::comms::send_to_daemon(
                                    crate::comms::DaemonCommand::SetFanSpeed { ac, rpm },
                                    socket,
                                ));
                            if let Ok(mut s) = tray.state.lock() {
                                s.fan_speed = Some(rpm);
                            }
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }),
            ksni::MenuItem::Separator,
        ];

        // Status section
        for line in [cpu_line, igpu_line, dgpu_line, bat_line].into_iter().flatten() {
            items.push(ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: line, enabled: false, ..Default::default()
            }));
        }

        items.push(ksni::MenuItem::Separator);
        items.push(ksni::MenuItem::Standard(ksni::menu::StandardItem {
            label: "Quit".into(),
            activate: Box::new(|_| std::process::exit(0)),
            ..Default::default()
        }));

        items
    }
}

// --- Sensor reading functions (standalone, no daemon dependency) ---

fn read_cpu_temp() -> Option<f64> {
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                let name = name.trim();
                if name == "k10temp" || name == "zenpower" || name == "coretemp" {
                    let temp_path = entry.path().join("temp1_input");
                    if let Ok(content) = fs::read_to_string(&temp_path) {
                        if let Ok(temp) = content.trim().parse::<f64>() {
                            return Some(temp / 1000.0);
                        }
                    }
                }
            }
        }
    }
    for path in ["/sys/class/thermal/thermal_zone0/temp", "/sys/class/thermal/thermal_zone1/temp"] {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(temp) = content.trim().parse::<f64>() {
                return Some(temp / 1000.0);
            }
        }
    }
    None
}

fn read_igpu_temp() -> Option<f64> {
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                if name.trim() == "amdgpu" {
                    for f in ["temp1_input", "temp2_input"] {
                        let p = entry.path().join(f);
                        if let Ok(c) = fs::read_to_string(&p) {
                            if let Ok(t) = c.trim().parse::<f64>() {
                                return Some(t / 1000.0);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn read_dgpu_temp() -> Option<f64> {
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"])
        .output()
    {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(t) = s.trim().parse::<f64>() {
                    return Some(t);
                }
            }
        }
    }
    None
}

fn read_ac_power() -> Option<bool> {
    for name in ["AC0", "ADP0", "ADP1", "ACAD"] {
        let path = format!("/sys/class/power_supply/{}/online", name);
        if let Ok(content) = fs::read_to_string(&path) {
            return Some(content.trim() == "1");
        }
    }
    None
}

fn read_battery_pct() -> Option<u8> {
    for bat in ["BAT0", "BAT1"] {
        let path = format!("/sys/class/power_supply/{}/capacity", bat);
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(pct) = content.trim().parse::<u8>() {
                return Some(pct);
            }
        }
    }
    None
}

fn read_system_power() -> Option<f64> {
    let paths = [
        "/sys/class/powercap/amd-rapl:0/energy_uj",
        "/sys/class/powercap/amd_rapl/amd-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl/intel-rapl:0/energy_uj",
    ];
    for path in &paths {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(energy) = content.trim().parse::<u64>() {
                use std::sync::atomic::{AtomicU64, Ordering};
                static LAST_E: AtomicU64 = AtomicU64::new(0);
                static LAST_T: AtomicU64 = AtomicU64::new(0);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_micros() as u64;
                let pe = LAST_E.swap(energy, Ordering::Relaxed);
                let pt = LAST_T.swap(now, Ordering::Relaxed);
                if pe > 0 && pt > 0 && energy > pe {
                    let dt = now - pt;
                    if dt > 0 {
                        return Some((energy - pe) as f64 / dt as f64);
                    }
                }
                return None;
            }
        }
    }
    None
}

fn read_dgpu_power() -> Option<f64> {
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=power.draw", "--format=csv,noheader,nounits"])
        .output()
    {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(p) = s.trim().parse::<f64>() {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn read_dgpu_util() -> Option<u32> {
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
        .output()
    {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(u) = s.trim().parse::<u32>() {
                    return Some(u);
                }
            }
        }
    }
    None
}

fn read_igpu_power() -> Option<f64> {
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                if name.trim() == "amdgpu" {
                    let p = entry.path().join("power1_average");
                    if let Ok(c) = fs::read_to_string(&p) {
                        if let Ok(uw) = c.trim().parse::<f64>() {
                            return Some(uw / 1_000_000.0);
                        }
                    }
                }
            }
        }
    }
    None
}

fn read_igpu_util() -> Option<u32> {
    for card in ["card0", "card1", "card2"] {
        let busy_path = format!("/sys/class/drm/{}/device/gpu_busy_percent", card);
        if let Ok(content) = fs::read_to_string(&busy_path) {
            if let Ok(util) = content.trim().parse::<u32>() {
                let driver_path = format!("/sys/class/drm/{}/device/driver", card);
                if let Ok(link) = fs::read_link(&driver_path) {
                    if link.to_string_lossy().contains("amdgpu") {
                        return Some(util);
                    }
                }
            }
        }
    }
    None
}

fn read_battery_status() -> Option<String> {
    for bat in ["BAT0", "BAT1"] {
        let path = format!("/sys/class/power_supply/{}/status", bat);
        if let Ok(content) = fs::read_to_string(&path) {
            let s = content.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn read_battery_power() -> Option<f64> {
    for bat in ["BAT0", "BAT1"] {
        let c_path = format!("/sys/class/power_supply/{}/current_now", bat);
        let v_path = format!("/sys/class/power_supply/{}/voltage_now", bat);
        if let (Ok(c_str), Ok(v_str)) = (fs::read_to_string(&c_path), fs::read_to_string(&v_path)) {
            if let (Ok(c), Ok(v)) = (c_str.trim().parse::<u64>(), v_str.trim().parse::<u64>()) {
                if c > 0 {
                    return Some(c as f64 * v as f64 / 1e12);
                }
            }
        }
    }
    None
}

fn read_cpu_util() -> Option<u32> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_IDLE: AtomicU64 = AtomicU64::new(0);
    static LAST_TOTAL: AtomicU64 = AtomicU64::new(0);

    if let Ok(content) = fs::read_to_string("/proc/stat") {
        if let Some(line) = content.lines().next() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 5 && fields[0] == "cpu" {
                let mut total: u64 = 0;
                for f in &fields[1..] {
                    if let Ok(v) = f.parse::<u64>() { total += v; }
                }
                let idle = fields[4].parse::<u64>().unwrap_or(0);
                let prev_idle = LAST_IDLE.swap(idle, Ordering::Relaxed);
                let prev_total = LAST_TOTAL.swap(total, Ordering::Relaxed);
                if prev_total > 0 {
                    let d_idle = idle.wrapping_sub(prev_idle);
                    let d_total = total.wrapping_sub(prev_total);
                    if d_total > 0 {
                        return Some((100.0 * (1.0 - d_idle as f64 / d_total as f64)).round() as u32);
                    }
                }
            }
        }
    }
    None
}
