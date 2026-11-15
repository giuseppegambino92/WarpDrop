//! Desktop → Android over a USB cable without any network: find devices with
//! `adb`, `adb forward tcp:0 tcp:<tunnel port>`, and run QUIC through the
//! resulting TCP pipe (see [`super::sockets::TunnelSocket`]).

use std::path::PathBuf;
use std::process::Command;

pub fn find_adb() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut cands: Vec<PathBuf> = vec![];
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(h) = std::env::var(var) {
            cands.push(PathBuf::from(h).join("platform-tools").join(exe));
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            cands.push(dir.join(exe));
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        cands.push(home.join("Library/Android/sdk/platform-tools").join(exe));
        cands.push(home.join("Android/Sdk/platform-tools").join(exe));
        cands.push(home.join("AppData/Local/Android/Sdk/platform-tools").join(exe));
    }
    cands.into_iter().find(|p| p.is_file())
}

/// Serials of devices in the `device` state (authorised, online).
pub fn devices(adb: &PathBuf) -> Vec<String> {
    let Ok(out) = Command::new(adb).arg("devices").output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            match (it.next(), it.next()) {
                (Some(serial), Some("device")) => Some(serial.to_string()),
                _ => None,
            }
        })
        .collect()
}

/// Creates (or reuses) a forward to the device's tunnel port; returns the
/// local TCP port.
pub fn forward(adb: &PathBuf, serial: &str, remote_port: u16) -> Option<u16> {
    let remote = format!("tcp:{remote_port}");
    if let Ok(out) = Command::new(adb).args(["-s", serial, "forward", "--list"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 3 && parts[0] == serial && parts[2] == remote {
                if let Some(p) = parts[1].strip_prefix("tcp:").and_then(|p| p.parse().ok()) {
                    return Some(p);
                }
            }
        }
    }
    let out = Command::new(adb).args(["-s", serial, "forward", "tcp:0", &remote]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}
