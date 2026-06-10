//! This is duplicated stuff for now, until we have a proper project structure

use serde::{Serialize, Deserialize};

const DEVICE_FILE_DEFAULT: &str = "/usr/share/razercontrol/laptops.json";

/// Logo LED states as encoded in the daemon protocol: index = logo_state value.
pub const LOGO_LABELS: [&str; 3] = ["Off", "On", "Breathing"];

/// Fallback fan range (RPM) when a device entry doesn't declare one.
pub const DEFAULT_FAN_MIN: i32 = 3500;
pub const DEFAULT_FAN_MAX: i32 = 5000;

pub fn device_file_path() -> String {
    std::env::var("RAZER_DEVICE_FILE").unwrap_or_else(|_| DEVICE_FILE_DEFAULT.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedDevice {
    pub name: String,
    pub vid: String,
    pub pid: String,
    pub features: Vec<String>,
    pub fan: Vec<u16>,
}

impl SupportedDevice {

    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    pub fn can_boost(&self) -> bool {
        self.has_feature("boost")
    }

    pub fn has_logo(&self) -> bool {
        self.has_feature("logo")
    }

}
