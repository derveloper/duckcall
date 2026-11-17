//! Everything macOS-specific. A Linux port provides the same items in `linux.rs`.

use crate::Result;
use crate::apps::App;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Handy's keyboard implementation on macOS before duckcall changes it.
pub const HANDY_KEYBOARD: &str = "handy_keys";

/// Left Option + Space the way Discord stores a shortcut: [keyboard, USB HID usage, macOS].
pub const DISCORD_OPTION_SPACE: [[u32; 3]; 2] = [[0, 226, 2], [0, 44, 2]];

const LABEL: &str = "io.github.lansidev.duckcall";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where the app keeps the settings duckcall changes.
pub fn store(app: App) -> PathBuf {
    home().join("Library/Application Support").join(match app {
        App::Handy => "com.pais.handy/settings_store.json",
        App::Fluxer => "fluxer/desktop-app-store.sqlite3",
        App::Discord => "discord/Local Storage/leveldb",
    })
}

pub fn handy_log() -> PathBuf {
    std::env::var_os("HANDY_LOG").map_or_else(|| home().join("Library/Logs/com.pais.handy/handy.log"), PathBuf::from)
}

// --- output volume -------------------------------------------------------------------------------
// ponytail: ducks the whole system output, not only the call app, and every change spawns
// osascript (~70 ms). Per-app volume needs a Core Audio process tap (audio-capture permission).

/// Lowers the output volume to `level` and returns what it was before.
/// `None` if the output has no volume control (HDMI): there is nothing to restore then.
pub fn duck_volume(level: u8) -> Option<u8> {
    let output = Command::new("osascript")
        .args(["-e", "set v to output volume of (get volume settings)"])
        .args(["-e", &format!("if v is not missing value and v > {level} then set volume output volume {level}")])
        .args(["-e", "v"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

pub fn set_volume(volume: u8) {
    let _ = Command::new("osascript").args(["-e", &format!("set volume output volume {volume}")]).status();
}

// --- apps ----------------------------------------------------------------------------------------

fn process(app: App) -> &'static str {
    match app {
        App::Handy => "handy",
        App::Fluxer => "Fluxer",
        App::Discord => "Discord",
    }
}

pub fn is_running(app: App) -> bool {
    Command::new("pgrep").args(["-x", process(app)]).stdout(Stdio::null()).status().is_ok_and(|status| status.success())
}

/// Asks the app to quit and waits until it is gone.
pub fn stop(app: App) -> Result<()> {
    Command::new("pkill").args(["-x", process(app)]).status()?;
    for _ in 0..150 {
        if !is_running(app) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("{} did not quit", app.name()).into())
}

pub fn start(app: App) -> Result<()> {
    Command::new("open").args(["-g", "-a", app.name()]).status()?;
    Ok(())
}

// --- background service: a LaunchAgent -----------------------------------------------------------

fn plist() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

fn launchctl(args: &[&str]) -> bool {
    Command::new("launchctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn service_stop() {
    let service = format!("{}/{LABEL}", domain());
    launchctl(&["bootout", &service]);
    // bootout returns before the service is gone, and bootstrap fails while it still exists.
    for _ in 0..50 {
        if !launchctl(&["print", &service]) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn agent(exe: &str, level: u8) -> String {
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>run</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>DUCK_VOLUME</key>
		<string>{level}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
</dict>
</plist>
"#
    )
}

/// Runs `duckcall run` at login and keeps it alive.
pub fn service_install(level: u8) -> Result<()> {
    let plist = plist();
    fs::create_dir_all(home().join("Library/LaunchAgents"))?;
    fs::write(&plist, agent(&std::env::current_exe()?.to_string_lossy(), level))?;
    service_stop();
    if !launchctl(&["bootstrap", &domain(), &plist.to_string_lossy()]) {
        return Err("launchctl could not start the background service".into());
    }
    Ok(())
}

pub fn service_remove() -> Result<()> {
    service_stop();
    match fs::remove_file(plist()) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plutil(file: &str, key: &str) -> String {
        let output = Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-", file]).output().unwrap();
        assert!(output.status.success(), "plutil cannot read {key}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn the_launch_agent_is_a_valid_plist_and_keeps_odd_paths_intact() {
        let exe = "/Users/R&D <dev>/bin/duckcall";
        let file = std::env::temp_dir().join("duckcall-test-agent.plist");
        fs::write(&file, agent(exe, 7)).unwrap();
        let file = file.to_string_lossy();

        assert_eq!(plutil(&file, "ProgramArguments.0"), exe);
        assert_eq!(plutil(&file, "ProgramArguments.1"), "run");
        assert_eq!(plutil(&file, "EnvironmentVariables.DUCK_VOLUME"), "7");
        assert_eq!(plutil(&file, "RunAtLoad"), "true");
        assert_eq!(plutil(&file, "KeepAlive"), "true");
    }
}
