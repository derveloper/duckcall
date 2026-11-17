//! Runs the real binary in a sandbox. `HOME` points at a scratch directory with fixture stores,
//! and the only `PATH` entry holds stand-ins for launchctl, pgrep, pkill, open, osascript and codesign,
//! so nothing here touches the real apps, the real service or the real volume.

use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

const DISCORD_KEYBINDS: &[u8] = b"_https://discord.com\x00\x01keybinds";
const STARTED: &str = "[2026-11-17][08:40:29][handy_app_lib::actions][DEBUG] TranscribeAction::start called for binding: transcribe";
const STOPPED: &str = "[2026-11-17][08:40:31][handy_app_lib::actions][DEBUG] TranscribeAction::stop called for binding: transcribe";

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("duckcall-cli-{name}"));
        fs::remove_dir_all(&root).ok();
        for dir in ["bin", "home", "running", "tmp"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        // Every stand-in notes its call; the rest mimics what duckcall relies on.
        let note = r#"echo "${0##*/} $*" >> "$SANDBOX/calls""#;
        let stand_in = |name: &str, script: String| {
            let path = root.join("bin").join(name);
            fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        stand_in("launchctl", format!("{note}\n[ \"$1\" != print ]")); // no service is ever loaded
        stand_in("pgrep", r#"[ -e "$SANDBOX/running/$2" ]"#.to_string());
        stand_in("pkill", format!("{note}\n/bin/rm -f \"$SANDBOX/running/$2\"")); // the app quits at once
        stand_in("open", note.to_string());
        stand_in("codesign", note.to_string());
        // Like the real osascript, only the script that asks for the volume prints something.
        let answer = r#"case "$*" in *"get volume settings"*) read -r volume < "$SANDBOX/volume"; echo "$volume" ;; esac"#;
        stand_in("osascript", format!("{note}\n{answer}"));
        let sandbox = Sandbox { root };
        sandbox.volume("40");
        sandbox
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn agent(&self) -> PathBuf {
        self.home().join("Library/LaunchAgents/io.github.lansidev.duckcall.plist")
    }

    /// The app bundle the service runs from.
    fn bundle(&self) -> PathBuf {
        self.home().join("Library/Application Support/duckcall/Duckcall.app")
    }

    fn store(&self, path: &str) -> PathBuf {
        let store = self.home().join("Library/Application Support").join(path);
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        store
    }

    fn handy_store(&self) -> PathBuf {
        self.store("com.pais.handy/settings_store.json")
    }

    fn fluxer_store(&self) -> PathBuf {
        self.store("fluxer/desktop-app-store.sqlite3")
    }

    fn discord_store(&self) -> PathBuf {
        self.store("discord/Local Storage/leveldb")
    }

    /// Handy as installed: its own keyboard implementation and a quiet log.
    fn handy(&self, shortcut: &str) -> &Sandbox {
        let settings = json!({ "settings": {
            "keyboard_implementation": "handy_keys",
            "log_level": "info",
            "theme": "system",
            "bindings": { "transcribe": { "current_binding": shortcut } },
        }});
        fs::write(self.handy_store(), serde_json::to_string_pretty(&settings).unwrap()).unwrap();
        self
    }

    /// Fluxer after its first start: keybind settings without a custom keybind.
    fn fluxer(&self) -> &Sandbox {
        let db = Connection::open(self.fluxer_store()).unwrap();
        db.execute_batch(
            r#"CREATE TABLE scoped_storage (store TEXT NOT NULL, scope TEXT NOT NULL, entry_key TEXT NOT NULL,
                   value TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY(store, scope, entry_key));
               INSERT INTO scoped_storage VALUES ('app', 'global', 'Keybind',
                   '{"customKeybinds":[],"transmitMode":"voice_activity","__mps__":{"version":5}}', 1.0);"#,
        )
        .unwrap();
        self
    }

    /// Discord after its first start: only a keybind of its own.
    fn discord(&self) -> &Sandbox {
        let state = r#"{"_state":{"1":{"id":"1","enabled":true,"action":"SAVE_SCREENSHOT","shortcut":[[0,69,2]],"managed":true,"params":{}}},"_version":5}"#;
        let mut db = rusty_leveldb::DB::open(self.discord_store(), rusty_leveldb::Options::default()).unwrap();
        db.put(DISCORD_KEYBINDS, &[&[1][..], state.as_bytes()].concat()).unwrap();
        db.close().unwrap();
        self
    }

    /// Makes the stand-in for pgrep report the process as running.
    fn running(&self, process: &str) -> &Sandbox {
        fs::write(self.root.join("running").join(process), "").unwrap();
        self
    }

    /// What the stand-in for osascript reports as the output volume.
    fn volume(&self, volume: &str) {
        fs::write(self.root.join("volume"), format!("{volume}\n")).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_duckcall"));
        command
            .env_clear()
            .env("PATH", self.root.join("bin"))
            .env("HOME", self.home())
            .env("TMPDIR", self.root.join("tmp"))
            .env("SANDBOX", &self.root)
            .stdin(Stdio::null()); // no terminal: nobody can agree to a restart
        command
    }

    fn duckcall(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// The calls of the stand-ins, in order.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls")).unwrap_or_default().lines().map(String::from).collect()
    }

    /// The volume changes `duckcall run` asked for, in order.
    fn volume_changes(&self) -> Vec<String> {
        let scripts = self.calls().into_iter().filter_map(|call| call.strip_prefix("osascript -e ").map(String::from));
        scripts
            .map(|script| match script.strip_prefix("set volume output volume ") {
                Some(volume) => format!("restore {volume}"),
                None => {
                    let level = script.split("then set volume output volume ").nth(1).unwrap_or(&script);
                    format!("duck to {}", level.trim_end_matches(" -e v"))
                }
            })
            .collect()
    }

    fn handy_setting(&self, key: &str) -> Value {
        let settings: Value = serde_json::from_str(&fs::read_to_string(self.handy_store()).unwrap()).unwrap();
        settings["settings"][key].clone()
    }

    /// How many push-to-mute keybinds on Option+Space Fluxer would load.
    fn fluxer_mutes(&self) -> usize {
        let state: String = Connection::open(self.fluxer_store())
            .unwrap()
            .query_row("SELECT value FROM scoped_storage WHERE entry_key = 'Keybind'", [], |row| row.get(0))
            .unwrap();
        let state: Value = serde_json::from_str(&state).unwrap();
        let keybinds = state["customKeybinds"].as_array().unwrap().iter();
        keybinds
            .filter(|keybind| {
                let combo = &keybind["combo"];
                keybind["action"] == "voice_push_to_mute" && combo["code"] == "Space" && combo["alt"] == true && combo["global"] == true
            })
            .count()
    }

    /// How many push-to-mute keybinds on Option+Space Discord would load.
    fn discord_mutes(&self) -> usize {
        // Opening a LevelDB rewrites it: look at a copy, so the store stays comparable.
        let copy = self.root.join("tmp/discord-copy");
        fs::remove_dir_all(&copy).ok();
        fs::create_dir_all(&copy).unwrap();
        for entry in fs::read_dir(self.discord_store()).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
        }
        let mut db = rusty_leveldb::DB::open(&copy, rusty_leveldb::Options::default()).unwrap();
        let state: Value = serde_json::from_slice(&db.get(DISCORD_KEYBINDS).unwrap()[1..]).unwrap();
        let keybinds = state["_state"].as_object().unwrap().values();
        keybinds
            .filter(|keybind| keybind["action"] == "PUSH_TO_MUTE" && keybind["shortcut"] == json!([[0, 226, 2], [0, 44, 2]]))
            .count()
    }

    /// The bytes of every store, to tell whether a run wrote anything.
    fn stores(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = vec![self.handy_store(), self.fluxer_store()];
        if let Ok(entries) = fs::read_dir(self.discord_store()) {
            files.extend(entries.map(|entry| entry.unwrap().path()));
        }
        files.sort();
        files.into_iter().filter(|file| file.is_file()).map(|file| (file.clone(), fs::read(file).unwrap())).collect()
    }
}

fn service() -> String {
    format!("gui/{}/io.github.lansidev.duckcall", unsafe { libc::getuid() })
}

/// One value of a property list, read by the real plutil.
fn plist_value(file: &Path, key: &str) -> String {
    let output = Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(file).output().unwrap();
    assert!(output.status.success(), "{key} in {}: {}", file.display(), String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

// --- install -------------------------------------------------------------------------------------

#[test]
fn install_sets_up_a_fresh_machine_and_writes_nothing_the_second_time() {
    let sandbox = Sandbox::new("fresh");
    sandbox.handy("option+space").fluxer().discord();

    let first = sandbox.duckcall(&["install"]);

    assert!(first.status.success(), "{first:?}");
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri");
    assert_eq!(sandbox.handy_setting("log_level"), "debug");
    assert_eq!(sandbox.handy_setting("theme"), "system");
    assert_eq!(sandbox.fluxer_mutes(), 1);
    assert_eq!(sandbox.discord_mutes(), 1);
    let bootstrap = format!("launchctl bootstrap gui/{} {}", unsafe { libc::getuid() }, sandbox.agent().display());
    assert!(sandbox.calls().contains(&bootstrap), "{:?}", sandbox.calls());
    assert!(sandbox.agent().is_file());

    let stores = sandbox.stores();
    let second = sandbox.duckcall(&["install"]);

    assert!(second.status.success(), "{second:?}");
    assert!(sandbox.stores() == stores, "the second install wrote to a store");
}

#[test]
fn install_stops_before_changing_anything_when_handy_has_no_settings() {
    let sandbox = Sandbox::new("no-handy");
    sandbox.fluxer().discord();
    let stores = sandbox.stores();

    let output = sandbox.duckcall(&["install"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(sandbox.stores() == stores, "a store was written");
    assert!(!sandbox.agent().exists());
    assert!(sandbox.calls().is_empty(), "{:?}", sandbox.calls());
}

#[test]
fn install_works_without_call_apps_and_creates_no_stores_for_them() {
    let sandbox = Sandbox::new("handy-only");
    sandbox.handy("option+space");

    let output = sandbox.duckcall(&["install"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri");
    assert!(sandbox.agent().is_file());
    assert!(!sandbox.fluxer_store().exists());
    assert!(!sandbox.discord_store().exists());
}

#[test]
fn install_leaves_the_call_apps_alone_when_handy_uses_another_shortcut() {
    let sandbox = Sandbox::new("other-shortcut");
    sandbox.handy("ctrl+shift+d").fluxer().discord();

    let output = sandbox.duckcall(&["install"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri");
    assert_eq!(sandbox.fluxer_mutes(), 0);
    assert_eq!(sandbox.discord_mutes(), 0);
}

#[test]
fn install_never_restarts_a_running_app_without_consent() {
    let sandbox = Sandbox::new("no-consent");
    sandbox.handy("option+space").fluxer().discord().running("Fluxer").running("Discord");

    let output = sandbox.duckcall(&["install"]);

    assert_eq!(output.status.code(), Some(1)); // not everything is set up
    assert_eq!(sandbox.fluxer_mutes(), 0);
    assert_eq!(sandbox.discord_mutes(), 0);
    let calls = sandbox.calls();
    assert!(!calls.iter().any(|call| call.starts_with("pkill") || call.starts_with("open")), "{calls:?}");
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri"); // Handy was not running
    assert!(sandbox.agent().is_file());
}

#[test]
fn install_yes_stops_running_apps_for_the_change_and_starts_them_again() {
    let sandbox = Sandbox::new("yes");
    sandbox.handy("option+space").fluxer().discord().running("handy").running("Fluxer").running("Discord");

    let output = sandbox.duckcall(&["install", "--yes"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri");
    assert_eq!(sandbox.fluxer_mutes(), 1);
    assert_eq!(sandbox.discord_mutes(), 1);
    let calls = sandbox.calls();
    for (process, app) in [("handy", "Handy"), ("Fluxer", "Fluxer"), ("Discord", "Discord")] {
        let stopped = calls.iter().position(|call| *call == format!("pkill -x {process}"));
        let started = calls.iter().position(|call| *call == format!("open -g -a {app}"));
        assert!(stopped.is_some() && stopped < started, "{app}: {calls:?}");
    }
}

#[test]
fn install_hands_the_ducking_level_to_the_service() {
    let sandbox = Sandbox::new("level");
    sandbox.handy("option+space");

    let output = sandbox.command().arg("install").env("DUCK_VOLUME", "12").output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(plist_value(&sandbox.agent(), "EnvironmentVariables.DUCK_VOLUME"), "12");
}

#[test]
fn install_refuses_a_ducking_level_that_is_no_volume() {
    let sandbox = Sandbox::new("bad-level");
    sandbox.handy("option+space");

    let output = sandbox.command().arg("install").env("DUCK_VOLUME", "loud").output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "handy_keys");
    assert!(!sandbox.agent().exists());
}

#[test]
fn install_runs_the_service_from_an_app_bundle_with_a_name_and_an_icon() {
    let sandbox = Sandbox::new("bundle");
    sandbox.handy("option+space");

    let output = sandbox.duckcall(&["install"]);

    assert!(output.status.success(), "{output:?}");
    let contents = sandbox.bundle().join("Contents");
    let info = contents.join("Info.plist");
    assert_eq!(plist_value(&info, "CFBundleName"), "Duckcall"); // what System Settings lists
    // The names in Info.plist must lead to the files that are there.
    let executable = contents.join("MacOS").join(plist_value(&info, "CFBundleExecutable"));
    assert!(fs::read(&executable).unwrap() == fs::read(env!("CARGO_BIN_EXE_duckcall")).unwrap(), "not this duckcall");
    let icon = contents.join("Resources").join(plist_value(&info, "CFBundleIconFile") + ".icns");
    let logo = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/duckcall.icns");
    assert!(fs::read(&icon).unwrap() == fs::read(logo).unwrap(), "not the logo");
    // macOS names the background item after the app that contains the service's executable.
    assert_eq!(plist_value(&sandbox.agent(), "ProgramArguments.0"), executable.to_string_lossy());
    let signed = format!("codesign --force --sign - {}", sandbox.bundle().display());
    assert!(sandbox.calls().contains(&signed), "{:?}", sandbox.calls());
}

// --- uninstall -----------------------------------------------------------------------------------

#[test]
fn uninstall_removes_the_service_and_gives_handy_its_keyboard_back() {
    let sandbox = Sandbox::new("uninstall");
    sandbox.handy("option+space").fluxer().discord();
    assert!(sandbox.duckcall(&["install"]).status.success());

    let output = sandbox.duckcall(&["uninstall"]);

    assert!(output.status.success(), "{output:?}");
    assert!(!sandbox.agent().exists());
    assert!(!sandbox.bundle().parent().unwrap().exists(), "the app bundle is still there");
    assert!(sandbox.calls().contains(&format!("launchctl bootout {}", service())));
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "handy_keys");
    assert_eq!(sandbox.fluxer_mutes(), 1); // the shortcuts stay
    assert_eq!(sandbox.discord_mutes(), 1);
}

#[test]
fn uninstall_does_not_restart_a_running_handy_without_consent() {
    let sandbox = Sandbox::new("uninstall-running");
    sandbox.handy("option+space");
    assert!(sandbox.duckcall(&["install"]).status.success());
    sandbox.running("handy");

    let output = sandbox.duckcall(&["uninstall"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "tauri");
    assert!(!sandbox.agent().exists()); // the service is gone all the same
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("pkill")));
}

// --- run -----------------------------------------------------------------------------------------

/// A `duckcall run` that is killed when the test ends, however it ends.
struct Service(Child);

impl Service {
    fn start(sandbox: &Sandbox, level: &str) -> (Service, impl Fn(&str)) {
        let log = sandbox.root.join("handy.log");
        fs::write(&log, "").unwrap();
        let child = sandbox.command().arg("run").env("HANDY_LOG", &log).env("DUCK_VOLUME", level).spawn().unwrap();
        let handy_logs = move |line: &str| {
            writeln!(fs::OpenOptions::new().append(true).open(&log).unwrap(), "{line}").unwrap();
        };
        (Service(child), handy_logs)
    }

    /// Stops it the way launchd does.
    fn stop(mut self) -> ExitStatus {
        unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) };
        let mut status = None;
        until("duckcall run to stop", || {
            status = self.0.try_wait().unwrap();
            status.is_some()
        });
        status.unwrap()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn run_ducks_while_handy_records_and_restores_on_stop_and_on_sigterm() {
    let sandbox = Sandbox::new("run");
    let (service, handy_logs) = Service::start(&sandbox, "7");

    // A repeated line changes nothing, so each one is written until the service has reacted.
    until("the volume to be ducked", || {
        handy_logs(STARTED);
        sandbox.volume_changes().len() == 1
    });
    until("the volume to be restored", || {
        handy_logs(STOPPED);
        sandbox.volume_changes().len() == 2
    });
    until("the volume to be ducked again", || {
        handy_logs(STARTED);
        sandbox.volume_changes().len() == 3
    });
    let status = service.stop();

    assert!(status.success(), "{status}");
    assert_eq!(sandbox.volume_changes(), ["duck to 7", "restore 40", "duck to 7", "restore 40"]);
}

#[test]
fn run_restores_nothing_on_an_output_without_volume_control() {
    let sandbox = Sandbox::new("run-hdmi");
    let (service, handy_logs) = Service::start(&sandbox, "5");
    until("the service to follow the log", || {
        handy_logs(STARTED);
        sandbox.volume_changes().len() == 1
    });
    until("the first recording to end", || {
        handy_logs(STOPPED);
        sandbox.volume_changes().len() == 2
    });

    sandbox.volume("missing value"); // what macOS reports for HDMI
    handy_logs(STARTED);
    until("the recording on HDMI", || sandbox.volume_changes().len() == 3);
    handy_logs(STOPPED);
    sandbox.volume("40");
    handy_logs(STARTED);
    until("the recording after HDMI", || sandbox.volume_changes().len() == 4);
    service.stop();

    let changes = sandbox.volume_changes();
    assert_eq!(changes, ["duck to 5", "restore 40", "duck to 5", "duck to 5", "restore 40"]);
}

// --- command line --------------------------------------------------------------------------------

#[test]
fn an_unknown_command_prints_the_usage_and_does_nothing() {
    let sandbox = Sandbox::new("usage");
    sandbox.handy("option+space");

    let output = sandbox.duckcall(&["instal"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: duckcall"));
    assert_eq!(sandbox.handy_setting("keyboard_implementation"), "handy_keys");
    assert!(sandbox.calls().is_empty());
}
