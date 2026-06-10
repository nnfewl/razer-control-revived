use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use libc::umask;

/// Razer laptop control socket path.
/// Prefer XDG_RUNTIME_DIR (/run/user/<uid>) which persists for the session.
/// Fall back to /tmp for AppImage or environments without XDG_RUNTIME_DIR.
pub fn socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{}/razercontrol-socket", dir)
    } else {
        "/tmp/razercontrol-socket".to_string()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub pci_slot: String,
    pub driver: String,
    pub gpu_type: String,
    pub runtime_status: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
/// Represents data sent TO the daemon
///
/// New variants must be appended at the END — bincode encodes the variant
/// index, so inserting in the middle breaks every client/daemon pair that
/// is not updated in lockstep.
pub enum DaemonCommand {
    SetFanSpeed { ac: usize, rpm: i32 },      // Fan speed
    GetFanSpeed { ac: usize },                 // Get (Fan speed)
    SetPowerMode { ac: usize, pwr: u8, cpu: u8, gpu: u8}, // Power mode
    GetPwrLevel { ac: usize },                 // Get (Power mode)
    GetCPUBoost { ac: usize },                 // Get (CPU boost)
    GetGPUBoost { ac: usize },                 // Get (GPU boost)
    SetLogoLedState{ ac:usize, logo_state: u8 },
    GetLogoLedState { ac: usize },
    GetKeyboardRGB { layer: i32 }, // Layer ID
    SetEffect { name: String, params: Vec<u8> }, // Set keyboard colour
    SetStandardEffect { name: String, params: Vec<u8> }, // Set keyboard colour
    SetBrightness { ac:usize, val: u8 },
    SetIdle {ac: usize, val: u32 },
    GetBrightness { ac: usize },
    SetSync { sync: bool },
    GetSync (),
    SetBatteryHealthOptimizer { is_on: bool, threshold: u8 },
    GetBatteryHealthOptimizer (),
    GetDeviceName,
    GetActualFanRpm,
    GetStandardEffect,
    GetGpuStatus,
    SetDgpuRuntimePM { enabled: bool },
    SetGpuMode { mode: String },
    ProbeBatteryHealthOptimizer,
    GetFanSpeedAndLogo { ac: usize },          // Combined tray poll: one round-trip
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
/// Represents data sent back from Daemon after it receives
/// a command. New variants must be appended at the END (see DaemonCommand).
pub enum DaemonResponse {
    SetFanSpeed { result: bool },                    // Response
    GetFanSpeed { rpm: i32 },                        // Get (Fan speed)
    SetPowerMode { result: bool },                   // Response
    GetPwrLevel { pwr: u8 },                         // Get (Power mode)
    GetCPUBoost { cpu: u8 },                         // Get (CPU boost)
    GetGPUBoost { gpu: u8 },                         // Get (GPU boost)
    SetLogoLedState {result: bool },
    GetLogoLedState { logo_state: u8 },
    GetKeyboardRGB { layer: i32, rgbdata: Vec<u8> }, // Response (RGB) of 90 keys
    SetEffect { result: bool },                       // Set keyboard colour
    SetStandardEffect { result: bool },                       // Set keyboard colour
    SetBrightness { result: bool },
    SetIdle { result: bool },
    GetBrightness { result: u8 },
    SetSync { result: bool },
    GetSync { sync: bool },
    SetBatteryHealthOptimizer { result: bool },
    GetBatteryHealthOptimizer { is_on: bool, threshold: u8 },
    GetDeviceName { name: String },
    GetActualFanRpm { rpm: i32 },
    GetStandardEffect { effect: u8, params: Vec<u8> },
    GetGpuStatus {
        gpus: Vec<GpuInfo>,
        dgpu_runtime_pm: bool,
        envycontrol_mode: String,
        envycontrol_available: bool,
    },
    SetDgpuRuntimePM { result: bool },
    SetGpuMode { result: bool, message: String },
    Unsupported { feature: String, device: String },
    ProbeBatteryHealthOptimizer {
        device: String,
        responded: bool,
        status: u8,
        raw_value: u8,
        is_on: bool,
        threshold: u8,
    },
    GetFanSpeedAndLogo { rpm: i32, logo_state: u8 },
}

#[allow(dead_code)]
pub fn bind() -> Option<UnixStream> {
    if let Ok(socket) = UnixStream::connect(socket_path()) {
        return Some(socket);
    } else {
        return None;
    }
}

#[allow(dead_code)]
/// We use this from the app, but it should replace bind
pub fn try_bind() -> std::io::Result<UnixStream> {
    UnixStream::connect(socket_path())
}

#[allow(dead_code)]
pub fn create() -> Option<UnixListener> {
    let path = socket_path();
    if std::fs::metadata(&path).is_ok() {
        // Socket file exists — check if a daemon is actually listening
        if UnixStream::connect(&path).is_ok() {
            eprintln!("UNIX Socket already exists and a daemon is responding. Is another daemon running?");
            return None;
        }
        // Stale socket from a previous crash — remove it
        eprintln!("Removing stale socket file");
        if std::fs::remove_file(&path).is_err() {
            eprintln!("Could not remove stale socket file");
            return None;
        }
    }
    // Set permissive umask so non-root GUI/CLI can connect to the daemon socket
    let old_umask = unsafe { umask(0o000) };
    let result = UnixListener::bind(&path);
    unsafe { umask(old_umask) };
    match result {
        Ok(listener) => Some(listener),
        Err(e) => {
            eprintln!("Failed to bind socket: {}", e);
            None
        }
    }
}

#[allow(dead_code)]
pub fn send_to_daemon(command: DaemonCommand, mut sock: UnixStream) -> Option<DaemonResponse> {
    // Prevent blocking the GTK main thread forever if daemon is unresponsive
    let timeout = Some(std::time::Duration::from_secs(5));
    let _ = sock.set_read_timeout(timeout);
    let _ = sock.set_write_timeout(timeout);

    if let Ok(encoded) = bincode::serialize(&command) {
        if sock.write_all(&encoded).is_ok() {
            // Signal request EOF to daemon so it can read the full command.
            let _ = sock.shutdown(Shutdown::Write);

            let mut response = Vec::new();
            return match sock.read_to_end(&mut response) {
                Ok(readed) if readed > 0 => read_from_socked_resp(&response),
                Ok(_) => {
                    eprintln!("No response from daemon");
                    None
                }
                Err(error) => {
                    eprintln!("Read failed: {error}");
                    None
                }
            };
        } else {
            eprintln!("Socket write failed!");
        }
    }
    return None;
}

/// Deserializes incomming bytes in order to return
/// a `DaemonResponse`. None is returned if deserializing failed
fn read_from_socked_resp(bytes: &[u8]) -> Option<DaemonResponse> {
    match bincode::deserialize::<DaemonResponse>(bytes) {
        Ok(res) => {
            println!("RES: {:?}", res);
            return Some(res);
        }
        Err(e) => {
            println!("RES ERROR: {}", e);
            return None;
        }
    }
}

/// Deserializes incomming bytes in order to return
/// a `DaemonCommand`. None is returned if deserializing failed
#[allow(dead_code)]
pub fn read_from_socket_req(bytes: &[u8]) -> Option<DaemonCommand> {
    match bincode::deserialize::<DaemonCommand>(bytes) {
        Ok(res) => {
            println!("REQ: {:?}", res);
            return Some(res);
        }
        Err(e) => {
            println!("REQ ERROR: {}", e);
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_cmd(cmd: DaemonCommand) {
        let bytes = bincode::serialize(&cmd).expect("serialize command");
        assert_eq!(bincode::deserialize::<DaemonCommand>(&bytes).expect("deserialize command"), cmd);
    }

    fn roundtrip_resp(resp: DaemonResponse) {
        let bytes = bincode::serialize(&resp).expect("serialize response");
        assert_eq!(bincode::deserialize::<DaemonResponse>(&bytes).expect("deserialize response"), resp);
    }

    #[test]
    fn fan_and_logo_status_roundtrips_through_bincode() {
        roundtrip_cmd(DaemonCommand::GetFanSpeedAndLogo { ac: 1 });
        roundtrip_resp(DaemonResponse::GetFanSpeedAndLogo { rpm: 4300, logo_state: 2 });
    }

    #[test]
    fn existing_tray_protocol_roundtrips_through_bincode() {
        roundtrip_cmd(DaemonCommand::GetFanSpeed { ac: 0 });
        roundtrip_cmd(DaemonCommand::SetFanSpeed { ac: 1, rpm: 4300 });
        roundtrip_cmd(DaemonCommand::GetLogoLedState { ac: 1 });
        roundtrip_cmd(DaemonCommand::SetLogoLedState { ac: 0, logo_state: 2 });
        roundtrip_cmd(DaemonCommand::GetDeviceName);
        roundtrip_resp(DaemonResponse::GetFanSpeed { rpm: 0 });
        roundtrip_resp(DaemonResponse::SetFanSpeed { result: true });
        roundtrip_resp(DaemonResponse::GetLogoLedState { logo_state: 1 });
        roundtrip_resp(DaemonResponse::SetLogoLedState { result: false });
        roundtrip_resp(DaemonResponse::GetDeviceName { name: "Blade 15".into() });
    }
}
