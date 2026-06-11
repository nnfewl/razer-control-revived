use std::fs;
use std::sync::{Arc, Mutex};
use serde_json;
use service::{DEFAULT_FAN_MAX, DEFAULT_FAN_MIN, LOGO_LABELS};

/// Static device capabilities, fetched once at startup — kept separate from
/// the per-poll sensor readings so the poll loop preserves them as one unit
/// and a newly added capability can't be silently reset every cycle.
#[derive(Clone, PartialEq, Debug)]
pub struct DeviceCaps {
    pub fan_min: i32,
    pub fan_max: i32,
    pub has_logo: bool,
}

impl Default for DeviceCaps {
    fn default() -> Self {
        DeviceCaps {
            fan_min: DEFAULT_FAN_MIN,
            fan_max: DEFAULT_FAN_MAX,
            has_logo: false,
        }
    }
}

#[derive(Clone, PartialEq)]
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
    pub logo_state: Option<u8>,
    pub caps: DeviceCaps,
}

impl Default for SensorState {
    fn default() -> Self {
        SensorState {
            cpu_temp: None, igpu_temp: None, dgpu_temp: None,
            fan_speed: None, on_ac: None, battery_pct: None,
            battery_status: None, battery_power: None, system_power: None,
            cpu_util: None, igpu_power: None, igpu_util: None,
            dgpu_power: None, dgpu_util: None,
            logo_state: None, caps: DeviceCaps::default(),
        }
    }
}

impl SensorState {
    /// Read all sensors directly from sysfs/nvidia-smi
    fn read_fresh() -> Self {
        let (dgpu_temp, dgpu_power, dgpu_util) = read_dgpu_stats();
        SensorState {
            cpu_temp: read_cpu_temp(),
            igpu_temp: read_igpu_temp(),
            dgpu_temp,
            fan_speed: None, // requires daemon, skip in tray
            on_ac: read_ac_power(),
            battery_pct: read_battery_pct(),
            battery_status: read_battery_status(),
            battery_power: read_battery_power(),
            system_power: read_system_power(),
            cpu_util: read_cpu_util(),
            igpu_power: read_igpu_power(),
            igpu_util: read_igpu_util(),
            dgpu_power,
            dgpu_util,
            logo_state: None,
            caps: DeviceCaps::default(),
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


pub fn start_background_polling(
    state: SharedSensorState,
    on_update: impl Fn() + Send + 'static,
) {
    std::thread::spawn(move || {
        // Retry until daemon responds (connection may not be ready at app launch)
        let caps = loop {
            match try_get_device_info_from_daemon() {
                Some(caps) => break caps,
                None => std::thread::sleep(std::time::Duration::from_secs(2)),
            }
        };
        let has_logo = caps.has_logo;
        if let Ok(mut s) = state.lock() {
            s.caps = caps;
        }

        let mut prev_fan_speed: Option<i32> = None;
        let mut prev_logo_state: Option<u8> = None;
        let mut prev_on_ac: Option<bool> = None;
        loop {
            let fresh = SensorState::read_fresh();
            let on_ac = fresh.on_ac;
            let ac = fresh.on_ac.unwrap_or(true);
            let ac_idx = if ac { 1 } else { 0 };

            // One daemon round-trip for both values — the socket protocol is
            // one command per connection
            let status = query_daemon(crate::comms::DaemonCommand::GetFanSpeedAndLogo { ac: ac_idx })
                .and_then(|resp| match resp {
                    crate::comms::DaemonResponse::GetFanSpeedAndLogo { rpm, logo_state } =>
                        Some((rpm, logo_state)),
                    _ => None,
                });

            let (fan_speed, logo_state) = match status {
                Some((rpm, logo)) => (Some(rpm), if has_logo { Some(logo) } else { None }),
                // Daemons older than the combined command don't answer it —
                // fall back to the legacy per-value commands so a version-skewed
                // daemon (e.g. not yet restarted after an upgrade) keeps working
                None => {
                    let fan = query_daemon(crate::comms::DaemonCommand::GetFanSpeed { ac: ac_idx })
                        .and_then(|resp| match resp {
                            crate::comms::DaemonResponse::GetFanSpeed { rpm } => Some(rpm),
                            _ => None,
                        });
                    let logo = if has_logo {
                        query_daemon(crate::comms::DaemonCommand::GetLogoLedState { ac: ac_idx })
                            .and_then(|resp| match resp {
                                crate::comms::DaemonResponse::GetLogoLedState { logo_state } => Some(logo_state),
                                _ => None,
                            })
                    } else {
                        None
                    };
                    (fan, logo)
                }
            };

            if let Ok(mut s) = state.lock() {
                let caps = s.caps.clone();
                *s = SensorState { fan_speed, logo_state, caps, ..fresh };
            }

            // Only poke the tray host when something interactive changes — sending an
            // update every poll cycle causes KDE to close/reset the submenu while the
            // user is trying to interact with it. AC transitions count: the per-profile
            // fan/logo values shown in the menu depend on the active power profile.
            if fan_speed != prev_fan_speed || logo_state != prev_logo_state || on_ac != prev_on_ac {
                prev_fan_speed = fan_speed;
                prev_logo_state = logo_state;
                prev_on_ac = on_ac;
                on_update();
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// Returns `None` if the daemon is not reachable, the device capabilities on success.
fn try_get_device_info_from_daemon() -> Option<DeviceCaps> {
    // Preferred: the daemon serves its own parsed laptops.json entry, so the
    // tray cannot drift from the daemon's view of the device
    if let Some(crate::comms::DaemonResponse::GetDeviceCapabilities { features, fan, .. }) =
        query_daemon(crate::comms::DaemonCommand::GetDeviceCapabilities)
    {
        return Some(caps_from_parts(&features, &fan));
    }

    // Fallback for daemons predating GetDeviceCapabilities: ask for the
    // device name and look it up in the local laptops.json copy
    let name = query_daemon(crate::comms::DaemonCommand::GetDeviceName)
        .and_then(|resp| match resp {
            crate::comms::DaemonResponse::GetDeviceName { name } => Some(name),
            _ => None,
        })?;

    let path = service::device_file_path();
    let json = fs::read_to_string(&path).ok()?;
    let devices: Vec<service::SupportedDevice> = serde_json::from_str(&json).ok()?;

    device_info(&devices, &name).or_else(|| {
        eprintln!(
            "Tray: device '{}' not found in {}; using default fan range, logo control disabled",
            name, path
        );
        Some(DeviceCaps::default())
    })
}

// --- Pure tray-menu logic (unit-tested below) ---

/// Parse one line of `nvidia-smi --query-gpu=temperature.gpu,power.draw,utilization.gpu
/// --format=csv,noheader,nounits` output into (temp °C, power W, util %).
/// Fields the driver reports as "[N/A]" (e.g. while the dGPU sleeps) become None.
fn parse_dgpu_stats(output: &str) -> (Option<f64>, Option<f64>, Option<u32>) {
    let first_gpu = output.lines().next().unwrap_or("");
    let mut fields = first_gpu.split(',').map(str::trim);
    let temp = fields.next().and_then(|f| f.parse::<f64>().ok());
    let power = fields.next().and_then(|f| f.parse::<f64>().ok());
    let util = fields.next().and_then(|f| f.parse::<u32>().ok());
    (temp, power, util)
}

/// Capabilities from a device entry's feature tags and fan range.
fn caps_from_parts(features: &[String], fan: &[u16]) -> DeviceCaps {
    DeviceCaps {
        fan_min: fan.first().map(|&v| v as i32).unwrap_or(DEFAULT_FAN_MIN),
        fan_max: fan.get(1).map(|&v| v as i32).unwrap_or(DEFAULT_FAN_MAX),
        has_logo: features.iter().any(|f| f == "logo"),
    }
}

/// Fan range and logo capability for `name` from the parsed device list.
/// `None` when the device is not in the list — the caller decides the
/// fallback (and should log the miss).
fn device_info(devices: &[service::SupportedDevice], name: &str) -> Option<DeviceCaps> {
    let device = devices.iter().find(|d| d.name == name)?;
    Some(caps_from_parts(&device.features, &device.fan))
}

/// One request/response round-trip; the socket protocol is one command per
/// connection. `None` when the daemon is unreachable or does not understand
/// the command (older protocol version).
fn query_daemon(cmd: crate::comms::DaemonCommand) -> Option<crate::comms::DaemonResponse> {
    crate::comms::try_bind()
        .ok()
        .and_then(|socket| crate::comms::send_to_daemon(cmd, socket))
}

/// True when the daemon acknowledged a Set command — the tray must not
/// update its local state optimistically on failure (REVIEW_FINDINGS #4).
fn set_acknowledged(resp: &crate::comms::DaemonResponse) -> bool {
    matches!(resp,
        crate::comms::DaemonResponse::SetFanSpeed { result: true }
        | crate::comms::DaemonResponse::SetLogoLedState { result: true })
}

/// Five fan presets derived from the device fan range. RPMs snap to the
/// nearest 100 to match hardware granularity (clamp_fan divides by 100).
fn fan_presets(fan_min: i32, fan_max: i32) -> [(i32, &'static str); 5] {
    let range = fan_max - fan_min;
    let pct_rpm = |p: f64| -> i32 {
        ((fan_min as f64 + range as f64 * p) / 100.0).round() as i32 * 100
    };
    [
        (0,             "Auto"),
        (pct_rpm(0.25), "Low"),
        (pct_rpm(0.50), "Medium"),
        (pct_rpm(0.75), "High"),
        (fan_max,       "Max"),
    ]
}

/// Exact match only — no fuzzy nearest-preset selection. `None` fan speed
/// (daemon unreachable) selects nothing rather than looking like Auto.
fn selected_preset_index(presets: &[(i32, &'static str)], fan_speed: Option<i32>) -> Option<usize> {
    let current_rpm = fan_speed?;
    presets.iter().position(|(rpm, _)| *rpm == current_rpm)
}

/// Preset label on exact match, otherwise the fan position as a percentage
/// of the device range ("Auto" for non-positive RPM, "…" when the fan speed
/// is unknown because the daemon is unreachable).
fn fan_current_label(
    presets: &[(i32, &'static str)],
    selected: Option<usize>,
    fan_speed: Option<i32>,
    fan_min: i32,
    fan_max: i32,
) -> String {
    match (selected, fan_speed) {
        (Some(i), _) => presets[i].1.to_string(),
        (None, None) => "…".to_string(),
        (None, Some(rpm)) if rpm <= 0 => "Auto".to_string(),
        (None, Some(rpm)) => {
            let range = fan_max - fan_min;
            let pct = if range > 0 {
                ((rpm - fan_min) as f64 / range as f64 * 100.0).round() as i32
            } else { 0 };
            format!("{}%", pct.clamp(0, 100))
        }
    }
}

/// Label for the current logo state; "…" when the state is unknown
/// (daemon not yet polled) or out of range.
fn logo_label(logo_state: Option<u8>) -> &'static str {
    logo_state
        .and_then(|s| LOGO_LABELS.get(s as usize).copied())
        .unwrap_or("…")
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
        let fan_min = state.caps.fan_min;
        let fan_max = state.caps.fan_max;

        let presets = fan_presets(fan_min, fan_max);
        let selected = selected_preset_index(&presets, state.fan_speed);
        let current_label = fan_current_label(&presets, selected, state.fan_speed, fan_min, fan_max);
        let fan_submenu_label = format!("Fan Speed  ·  {}", current_label);

        // Status lines
        fn stat_line(name: &str, temp: Option<f64>, util: Option<u32>) -> Option<String> {
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = temp { parts.push(format!("{:.0}°C", t)); }
            if let Some(u) = util { parts.push(format!("{}%", u)); }
            if parts.is_empty() { return None; }
            Some(format!("{}  {}", name, parts.join(" · ")))
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

        let logo_state = state.logo_state;
        let has_logo = state.caps.has_logo;
        let logo_submenu_label = format!("Logo  ·  {}", logo_label(logo_state));

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
                    let display = label.to_string();
                    ksni::MenuItem::Checkmark(ksni::menu::CheckmarkItem {
                        label: display,
                        checked: selected == Some(i),
                        activate: Box::new(move |tray: &mut RazerTray| {
                            // Read AC state at click time — the menu snapshot can be stale
                            // (ksni only rebuilds it on handle.update(), not on open)
                            let on_ac = tray.state.lock().ok().and_then(|s| s.on_ac).unwrap_or(true);
                            let ac = if on_ac { 1 } else { 0 };
                            let acknowledged = crate::comms::try_bind()
                                .ok()
                                .and_then(|socket| crate::comms::send_to_daemon(
                                    crate::comms::DaemonCommand::SetFanSpeed { ac, rpm },
                                    socket,
                                ))
                                .is_some_and(|resp| set_acknowledged(&resp));
                            if acknowledged {
                                if let Ok(mut s) = tray.state.lock() {
                                    s.fan_speed = Some(rpm);
                                }
                            }
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }),
        ];

        // Logo submenu — only shown on devices with logo feature
        if has_logo {
            items.push(ksni::MenuItem::SubMenu(ksni::menu::SubMenu {
                label: logo_submenu_label,
                submenu: LOGO_LABELS.iter().enumerate().map(|(i, label)| {
                    let logo_mode = i as u8;
                    ksni::MenuItem::Checkmark(ksni::menu::CheckmarkItem {
                        label: label.to_string(),
                        checked: logo_state == Some(logo_mode),
                        activate: Box::new(move |tray: &mut RazerTray| {
                            // Read AC state at click time — the menu snapshot can be stale
                            // (ksni only rebuilds it on handle.update(), not on open)
                            let on_ac = tray.state.lock().ok().and_then(|s| s.on_ac).unwrap_or(true);
                            let ac = if on_ac { 1 } else { 0 };
                            let acknowledged = crate::comms::try_bind()
                                .ok()
                                .and_then(|socket| crate::comms::send_to_daemon(
                                    crate::comms::DaemonCommand::SetLogoLedState { ac, logo_state: logo_mode },
                                    socket,
                                ))
                                .is_some_and(|resp| set_acknowledged(&resp));
                            if acknowledged {
                                if let Ok(mut s) = tray.state.lock() {
                                    s.logo_state = Some(logo_mode);
                                }
                            }
                        }),
                        ..Default::default()
                    })
                }).collect(),
                ..Default::default()
            }));
        }

        items.push(ksni::MenuItem::Separator);

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
    // AMD / newer Intel: hwmon entry named "amdgpu" or "i915"
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                let name = name.trim();
                if name == "amdgpu" || name == "i915" {
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
    // Intel CometLake/IceLake: ACPI thermal zone "B0D4" is the iGPU die
    if let Ok(zones) = fs::read_dir("/sys/class/thermal") {
        for zone in zones.flatten() {
            let type_path = zone.path().join("type");
            if let Ok(t) = fs::read_to_string(&type_path) {
                if t.trim() == "B0D4" {
                    let temp_path = zone.path().join("temp");
                    if let Ok(c) = fs::read_to_string(&temp_path) {
                        if let Ok(temp) = c.trim().parse::<f64>() {
                            return Some(temp / 1000.0);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Temperature, power, and utilization in a single nvidia-smi invocation —
/// process startup dominates query time, and frequent spawns can keep the
/// dGPU awake on Optimus laptops.
fn read_dgpu_stats() -> (Option<f64>, Option<f64>, Option<u32>) {
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=temperature.gpu,power.draw,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if output.status.success() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                return parse_dgpu_stats(&s);
            }
        }
    }
    (None, None, None)
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

/// Compute watts from a RAPL energy counter reading.
///
/// Uses a monotonic clock (immune to NTP steps) and handles counter wraparound
/// via `max_range_uj` (from the sibling `max_energy_range_uj` sysfs file;
/// pass 0 to skip wraparound recovery). Returns `None` on the first call
/// (no baseline) or when the counter cannot be interpreted. µJ / µs = W.
pub(super) fn rapl_watts(
    energy_uj: u64,
    max_range_uj: u64,
    last_e: &std::sync::atomic::AtomicU64,
    last_t: &std::sync::atomic::AtomicU64,
) -> Option<f64> {
    use std::sync::atomic::Ordering;
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let now = BASE.get_or_init(std::time::Instant::now).elapsed().as_micros() as u64;
    let pe = last_e.swap(energy_uj, Ordering::Relaxed);
    let pt = last_t.swap(now, Ordering::Relaxed);
    if pe == 0 || pt == 0 || now <= pt { return None; }
    let delta_e = if energy_uj >= pe {
        energy_uj - pe
    } else if max_range_uj > 0 {
        (max_range_uj - pe) + energy_uj  // counter wrapped
    } else {
        return None;
    };
    let dt = now - pt;
    if dt == 0 { return None; }
    Some(delta_e as f64 / dt as f64)
}

fn rapl_read(path: &str, last_e: &std::sync::atomic::AtomicU64, last_t: &std::sync::atomic::AtomicU64) -> Option<f64> {
    let energy: u64 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    let max_range: u64 = fs::read_to_string(&path.replace("energy_uj", "max_energy_range_uj"))
        .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    rapl_watts(energy, max_range, last_e, last_t)
}

fn read_system_power() -> Option<f64> {
    use std::sync::atomic::AtomicU64;
    static LAST_E: AtomicU64 = AtomicU64::new(0);
    static LAST_T: AtomicU64 = AtomicU64::new(0);
    let paths = [
        "/sys/class/powercap/amd-rapl:0/energy_uj",
        "/sys/class/powercap/amd_rapl/amd-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl/intel-rapl:0/energy_uj",
    ];
    for path in &paths {
        if fs::metadata(path).is_ok() {
            return rapl_read(path, &LAST_E, &LAST_T);
        }
    }
    None
}

fn read_igpu_power() -> Option<f64> {
    use std::sync::atomic::AtomicU64;
    // AMD: hwmon "amdgpu" exposes instantaneous power directly
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
    // Intel: RAPL "uncore" domain = iGPU + LLC; closest available iGPU power source
    static LAST_E: AtomicU64 = AtomicU64::new(0);
    static LAST_T: AtomicU64 = AtomicU64::new(0);
    let rapl_candidates = [
        "/sys/class/powercap/intel-rapl:0:1/energy_uj",
        "/sys/class/powercap/intel-rapl-mmio:0:0/energy_uj",
    ];
    for path in &rapl_candidates {
        // Only use paths whose domain is "uncore" to avoid grabbing CPU core power
        let name_path = match std::path::Path::new(path).parent() {
            Some(p) => p.join("name"),
            None => continue,
        };
        let Ok(name_content) = fs::read_to_string(&name_path) else { continue };
        if name_content.trim() != "uncore" { continue; }
        if fs::metadata(path).is_ok() {
            return rapl_read(path, &LAST_E, &LAST_T);
        }
    }
    None
}

fn read_mhz(path: &str) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_igpu_util() -> Option<u32> {
    for card in ["card0", "card1", "card2"] {
        let driver_path = format!("/sys/class/drm/{}/device/driver", card);
        let Ok(link) = fs::read_link(&driver_path) else { continue };
        let driver = link.to_string_lossy();

        // AMD: kernel exposes gpu_busy_percent directly
        if driver.contains("amdgpu") {
            let busy_path = format!("/sys/class/drm/{}/device/gpu_busy_percent", card);
            if let Ok(content) = fs::read_to_string(&busy_path) {
                if let Ok(util) = content.trim().parse::<u32>() {
                    return Some(util);
                }
            }
        }

        // Intel: approximate utilization from active vs max frequency.
        // act_freq ≤ RPn means the GPU is in RC6 idle → 0%.
        if driver.contains("i915") {
            let base = format!("/sys/class/drm/{}", card);
            let Some(act) = read_mhz(&format!("{}/gt_act_freq_mhz", base)) else { continue };
            let Some(min) = read_mhz(&format!("{}/gt_RPn_freq_mhz", base)) else { continue };
            let Some(max) = read_mhz(&format!("{}/gt_RP0_freq_mhz", base)) else { continue };
            if max <= min {
                return Some(0);
            }
            let pct = ((act.saturating_sub(min)) as f64 / (max - min) as f64 * 100.0) as u32;
            return Some(pct.min(100));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_device(name: &str, features: &[&str], fan: &[u16]) -> service::SupportedDevice {
        service::SupportedDevice {
            name: name.into(),
            vid: "1532".into(),
            pid: "0000".into(),
            features: features.iter().map(|f| f.to_string()).collect(),
            fan: fan.to_vec(),
        }
    }

    #[test]
    fn set_commands_acknowledged_only_on_true_result() {
        use crate::comms::DaemonResponse;
        assert!(set_acknowledged(&DaemonResponse::SetFanSpeed { result: true }));
        assert!(set_acknowledged(&DaemonResponse::SetLogoLedState { result: true }));
        assert!(!set_acknowledged(&DaemonResponse::SetFanSpeed { result: false }));
        assert!(!set_acknowledged(&DaemonResponse::SetLogoLedState { result: false }));
        // unexpected response variant is not an ack
        assert!(!set_acknowledged(&DaemonResponse::GetFanSpeed { rpm: 0 }));
    }

    #[test]
    fn device_info_reads_fan_range_and_logo_feature() {
        let devices = [test_device("Blade 15", &["fan", "logo"], &[3200, 5200])];
        assert_eq!(
            device_info(&devices, "Blade 15"),
            Some(DeviceCaps { fan_min: 3200, fan_max: 5200, has_logo: true })
        );
    }

    #[test]
    fn device_info_defaults_fan_range_when_entry_has_none() {
        let devices = [test_device("Blade 14", &["fan"], &[])];
        assert_eq!(device_info(&devices, "Blade 14"), Some(DeviceCaps::default()));
    }

    #[test]
    fn device_info_is_none_when_device_not_in_list() {
        // Caller must be able to tell "unknown device" from "no fan data",
        // so it can log the mismatch (REVIEW_FINDINGS #1)
        let devices = [test_device("Blade 15", &["fan"], &[3200, 5200])];
        assert_eq!(device_info(&devices, "Blade 18"), None);
        assert_eq!(device_info(&[], "Blade 18"), None);
    }

    #[test]
    fn presets_for_default_range() {
        let p = fan_presets(3500, 5000);
        assert_eq!(p, [
            (0,    "Auto"),
            (3900, "Low"),
            (4300, "Medium"),
            (4600, "High"),
            (5000, "Max"),
        ]);
    }

    #[test]
    fn presets_snap_to_100_rpm_for_awkward_ranges() {
        for (min, max) in [(3100, 5300), (1000, 5500), (2300, 4700)] {
            for (rpm, label) in fan_presets(min, max) {
                assert_eq!(rpm % 100, 0,
                    "{label} preset {rpm} for range ({min},{max}) is not a multiple of 100");
            }
        }
    }

    #[test]
    fn preset_selection_is_exact_match_only() {
        let p = fan_presets(3500, 5000);
        assert_eq!(selected_preset_index(&p, Some(4300)), Some(2));
        assert_eq!(selected_preset_index(&p, Some(0)), Some(0)); // Auto
        assert_eq!(selected_preset_index(&p, Some(4000)), None); // no fuzzy matching
    }

    #[test]
    fn no_preset_checked_when_fan_speed_unknown() {
        // Daemon unreachable must not look like Auto mode (REVIEW_FINDINGS #2)
        let p = fan_presets(3500, 5000);
        assert_eq!(selected_preset_index(&p, None), None);
    }

    #[test]
    fn fan_label_shows_unknown_when_fan_speed_unknown() {
        let p = fan_presets(3500, 5000);
        assert_eq!(fan_current_label(&p, None, None, 3500, 5000), "…");
    }

    #[test]
    fn fan_label_uses_preset_name_on_exact_match() {
        let p = fan_presets(3500, 5000);
        assert_eq!(fan_current_label(&p, Some(3), Some(4600), 3500, 5000), "High");
    }

    #[test]
    fn fan_label_shows_percent_for_non_preset_rpm() {
        let p = fan_presets(3500, 5000);
        // (4000 - 3500) / 1500 = 33.3%
        assert_eq!(fan_current_label(&p, None, Some(4000), 3500, 5000), "33%");
    }

    #[test]
    fn fan_label_percent_is_clamped_and_division_safe() {
        let p = fan_presets(3500, 5000);
        assert_eq!(fan_current_label(&p, None, Some(5400), 3500, 5000), "100%");
        assert_eq!(fan_current_label(&p, None, Some(3400), 3500, 5000), "0%");
        // degenerate range (min == max) must not divide by zero
        assert_eq!(fan_current_label(&[], None, Some(4000), 4000, 4000), "0%");
    }

    #[test]
    fn fan_label_negative_rpm_reads_auto() {
        let p = fan_presets(3500, 5000);
        assert_eq!(fan_current_label(&p, None, Some(-1), 3500, 5000), "Auto");
    }

    #[test]
    fn parses_combined_nvidia_smi_output() {
        assert_eq!(parse_dgpu_stats("65, 35.50, 12\n"), (Some(65.0), Some(35.5), Some(12)));
    }

    #[test]
    fn dgpu_fields_reported_as_not_available_become_none() {
        // nvidia-smi emits "[N/A]" per field, e.g. while the dGPU is asleep
        assert_eq!(parse_dgpu_stats("65, [N/A], [N/A]\n"), (Some(65.0), None, None));
    }

    #[test]
    fn garbage_or_empty_dgpu_output_yields_all_none() {
        assert_eq!(parse_dgpu_stats(""), (None, None, None));
        assert_eq!(parse_dgpu_stats("NVIDIA-SMI has failed\n"), (None, None, None));
    }

    #[test]
    fn multi_gpu_output_uses_first_gpu() {
        assert_eq!(
            parse_dgpu_stats("65, 35.50, 12\n48, 10.00, 3\n"),
            (Some(65.0), Some(35.5), Some(12))
        );
    }

    #[test]
    fn logo_label_maps_known_states() {
        assert_eq!(logo_label(Some(0)), "Off");
        assert_eq!(logo_label(Some(1)), "On");
        assert_eq!(logo_label(Some(2)), "Breathing");
    }

    #[test]
    fn logo_label_shows_unknown_marker_for_unknown_state() {
        // Unknown state must not assert "On" — the daemon may not have
        // responded yet, or the state may be out of range (REVIEW_FINDINGS #3)
        assert_eq!(logo_label(None), "…");
        assert_eq!(logo_label(Some(7)), "…");
    }
}
