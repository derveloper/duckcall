//! Everything macOS-specific. A Linux port provides the same items in `linux.rs`.

use crate::Result;
use crate::apps::App;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
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

// --- background service: a LaunchAgent that runs from an app bundle ------------------------------

/// The icon of the app bundle, built from `assets/logo.svg` by `assets/icns.sh`.
const ICON: &[u8] = include_bytes!("../assets/duckcall.icns");

fn plist() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

/// The service runs from an app bundle of its own, because macOS names a background item after
/// the app that contains its executable: System Settings then lists "Duckcall" with its icon
/// instead of an anonymous executable. The documented way, `AssociatedBundleIdentifiers` in the
/// LaunchAgent, is ignored for executables without a Team ID.
fn bundle() -> PathBuf {
    home().join("Library/Application Support/duckcall/Duckcall.app")
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>{LABEL}</string>
	<key>CFBundleName</key>
	<string>Duckcall</string>
	<key>CFBundleDisplayName</key>
	<string>Duckcall</string>
	<key>CFBundleExecutable</key>
	<string>duckcall</string>
	<key>CFBundleIconFile</key>
	<string>duckcall</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSUIElement</key>
	<true/>
</dict>
</plist>
"#,
        version = env!("CARGO_PKG_VERSION")
    )
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Builds the app bundle around a copy of this executable and returns the copy.
fn bundle_install() -> Result<PathBuf> {
    let contents = bundle().join("Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    fs::write(contents.join("Info.plist"), info_plist())?;
    fs::write(contents.join("Resources/duckcall.icns"), ICON)?;
    let exe = contents.join("MacOS/duckcall");
    let this = fs::canonicalize(std::env::current_exe()?)?;
    // `install` may be run from the copy itself.
    if fs::canonicalize(&exe).ok().as_ref() != Some(&this) {
        remove_file(&exe)?; // a new file, never a rewrite of one that may still be mapped
        fs::copy(&this, &exe)?;
    }
    // Info.plist and the icon belong to the bundle's signature, so sign it as a whole.
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(bundle())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();
    if !signed {
        return Err("codesign could not sign the app bundle".into());
    }
    Ok(exe)
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
    service_stop(); // nothing may run from the bundle while it is rebuilt
    let exe = bundle_install()?;
    let plist = plist();
    fs::create_dir_all(home().join("Library/LaunchAgents"))?;
    fs::write(&plist, agent(&exe.to_string_lossy(), level))?;
    if !launchctl(&["bootstrap", &domain(), &plist.to_string_lossy()]) {
        return Err("launchctl could not start the background service".into());
    }
    Ok(())
}

pub fn service_remove() -> Result<()> {
    service_stop();
    remove_file(&plist())?;
    let bundle = bundle();
    if bundle.exists() {
        fs::remove_dir_all(&bundle)?;
    }
    // The directory around the bundle is duckcall's own: it goes too, unless something else is in it.
    if let Some(directory) = bundle.parent() {
        fs::remove_dir(directory).ok();
    }
    Ok(())
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
