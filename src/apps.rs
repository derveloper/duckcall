//! The settings duckcall needs in other apps. A store may only be written while its app is not running.

use crate::{Result, platform};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq)]
pub enum App {
    Handy,
    Fluxer,
    Discord,
}

pub enum Setup {
    /// Not installed, or never started: there are no settings to change yet.
    Absent,
    Done,
    Pending,
}

impl App {
    pub const ALL: [App; 3] = [App::Handy, App::Fluxer, App::Discord];

    /// Also the name the system starts the app by.
    pub fn name(self) -> &'static str {
        match self {
            App::Handy => "Handy",
            App::Fluxer => "Fluxer",
            App::Discord => "Discord",
        }
    }

    pub fn setup(self) -> Result<Setup> {
        let store = platform::store(self);
        if !store.exists() {
            return Ok(Setup::Absent);
        }
        let done = match self {
            App::Handy => Some(handy_done(&handy_read(&store)?)),
            App::Fluxer => fluxer_read(&store)?.map(|state| fluxer_done(&state)),
            App::Discord => discord_read(&store)?.map(|state| discord_done(&state)),
        };
        Ok(match done {
            None => Setup::Absent,
            Some(true) => Setup::Done,
            Some(false) => Setup::Pending,
        })
    }

    /// Writes what `setup` reported as pending. The app must not be running.
    pub fn configure(self) -> Result<()> {
        let store = platform::store(self);
        match self {
            App::Handy => handy_set(&store, &[("keyboard_implementation", "tauri"), ("log_level", "debug")]),
            App::Fluxer => fluxer_configure(&store),
            App::Discord => discord_configure(&store),
        }
    }
}

// --- Handy: a JSON file that Handy rewrites whenever a setting changes ---------------------------

fn handy_read(store: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(store)?)?)
}

/// "tauri": Handy registers a system hotkey instead of swallowing the key event, so a call app
/// listening for the same key still sees it. "debug": the log lines `run` follows are DEBUG.
fn handy_done(settings: &Value) -> bool {
    settings["settings"]["keyboard_implementation"] == "tauri" && settings["settings"]["log_level"] == "debug"
}

pub fn handy_set(store: &Path, values: &[(&str, &str)]) -> Result<()> {
    let mut settings = handy_read(store)?;
    let entries = settings.get_mut("settings").and_then(Value::as_object_mut).ok_or("unexpected Handy settings")?;
    for &(key, value) in values {
        entries.insert(key.to_string(), json!(value));
    }
    fs::write(store, serde_json::to_string_pretty(&settings)?)?;
    Ok(())
}

/// A string from Handy's settings by JSON pointer, e.g. "/settings/keyboard_implementation".
pub fn handy_get(pointer: &str) -> Result<String> {
    let store = platform::store(App::Handy);
    if !store.exists() {
        return Err("Handy settings not found; start Handy once and run this again".into());
    }
    Ok(handy_read(&store)?.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_string())
}

// --- Fluxer: a SQLite key-value store; this row is the persisted keybind state -------------------

const FLUXER_ROW: &str = "store = 'app' AND scope = 'global' AND entry_key = 'Keybind'";

/// Marks the keybind as written by duckcall.
const FLUXER_KEYBIND_ID: &str = "d0cca110-d0cc-4a11-8a11-d0cca11d0cca";

fn fluxer_read(store: &Path) -> Result<Option<Value>> {
    let db = Connection::open_with_flags(store, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let row: Option<String> = db
        .query_row(&format!("SELECT value FROM scoped_storage WHERE {FLUXER_ROW}"), [], |row| row.get(0))
        .optional()?;
    Ok(row.map(|state| serde_json::from_str(&state)).transpose()?)
}

/// Is there a working global push-to-mute on Option+Space?
fn fluxer_done(state: &Value) -> bool {
    state["customKeybinds"].as_array().is_some_and(|keybinds| {
        keybinds.iter().any(|keybind| {
            let combo = &keybind["combo"];
            keybind["action"] == "voice_push_to_mute"
                && keybind["enabled"] != false
                && combo["code"] == "Space"
                && combo["alt"] == true
                && combo["global"] == true
                && combo["enabled"] != false
                && ["ctrl", "shift", "meta", "ctrlOrMeta"].iter().all(|&modifier| combo[modifier] != true)
        })
    })
}

fn fluxer_configure(store: &Path) -> Result<()> {
    let mut state = fluxer_read(store)?.ok_or("Fluxer has no keybind settings yet")?;
    let keybinds = state
        .get_mut("customKeybinds")
        .and_then(Value::as_array_mut)
        .ok_or("unexpected Fluxer keybind settings")?;
    keybinds.push(json!({
        "id": FLUXER_KEYBIND_ID,
        "action": "voice_push_to_mute",
        "combo": { "key": " ", "code": "Space", "alt": true, "global": true, "enabled": true },
        "enabled": true,
    }));
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as f64;
    Connection::open(store)?.execute(
        &format!("UPDATE scoped_storage SET value = ?1, updated_at = ?2 WHERE {FLUXER_ROW}"),
        rusqlite::params![state.to_string(), now],
    )?;
    Ok(())
}

// --- Discord: Chromium localStorage, a LevelDB; "keybinds" is the persisted KeybindsStore --------

const DISCORD_KEYBINDS: &[u8] = b"_https://discord.com\x00\x01keybinds";

fn discord_open(dir: &Path) -> Result<rusty_leveldb::DB> {
    let options = rusty_leveldb::Options { create_if_missing: false, ..Default::default() };
    Ok(rusty_leveldb::DB::open(dir, options)?)
}

fn discord_parse(raw: &[u8]) -> Result<Value> {
    Ok(serde_json::from_str(&local_storage_decode(raw).ok_or("unreadable Discord keybind settings")?)?)
}

/// Opening a LevelDB replays its log and rewrites its manifest, and Discord may be running:
/// look at a copy.
fn discord_read(dir: &Path) -> Result<Option<Value>> {
    let copy = std::env::temp_dir().join(format!("duckcall-discord-{}", std::process::id()));
    let state = copy_dir(dir, &copy).and_then(|()| {
        let mut db = discord_open(&copy)?;
        db.get(DISCORD_KEYBINDS).map(|raw| discord_parse(&raw)).transpose()
    });
    fs::remove_dir_all(&copy).ok();
    state
}

/// Is there a push-to-mute on Option+Space?
fn discord_done(state: &Value) -> bool {
    state["_state"].as_object().is_some_and(|keybinds| {
        keybinds.values().any(|keybind| {
            keybind["action"] == "PUSH_TO_MUTE"
                && keybind["enabled"] != false
                && keybind["shortcut"] == json!(platform::DISCORD_OPTION_SPACE)
        })
    })
}

fn discord_configure(dir: &Path) -> Result<()> {
    // The same LevelDB holds the Discord login: keep the state from before the write next to it.
    let backup = dir.with_file_name("leveldb.duckcall-backup");
    fs::remove_dir_all(&backup).ok();
    copy_dir(dir, &backup)?;

    let mut db = discord_open(dir)?;
    let mut state = discord_parse(&db.get(DISCORD_KEYBINDS).ok_or("Discord has no keybind settings yet")?)?;
    let keybinds = state.get_mut("_state").and_then(Value::as_object_mut).ok_or("unexpected Discord keybind settings")?;
    // Discord numbers a new keybind as the highest id + 1, and drops ids that are not numbers.
    let id = keybinds.keys().filter_map(|id| id.parse::<u64>().ok()).max().map_or(0, |highest| highest + 1).to_string();
    keybinds.insert(
        id.clone(),
        json!({
            "id": id,
            "enabled": true,
            "action": "PUSH_TO_MUTE",
            "shortcut": platform::DISCORD_OPTION_SPACE,
            "managed": false,
            "params": {},
        }),
    );
    db.put(DISCORD_KEYBINDS, &local_storage_encode(&state.to_string()))?;
    db.flush()?;
    db.close()?;
    Ok(())
}

/// Chromium stores a localStorage value as a marker byte plus UTF-16LE (0) or Latin-1 (1) text.
fn local_storage_decode(raw: &[u8]) -> Option<String> {
    match raw.split_first()? {
        (0, text) => {
            let units: Vec<u16> = text.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
            String::from_utf16(&units).ok()
        }
        (1, text) => Some(text.iter().map(|&byte| byte as char).collect()),
        _ => None,
    }
}

fn local_storage_encode(text: &str) -> Vec<u8> {
    if text.chars().all(|c| c as u32 <= 0xFF) {
        std::iter::once(1).chain(text.chars().map(|c| c as u8)).collect()
    } else {
        std::iter::once(0).chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect()
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("duckcall-test-{name}"));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn handy_gets_the_settings_and_keeps_the_rest() {
        let store = scratch("handy").join("settings_store.json");
        fs::write(&store, r#"{"settings":{"keyboard_implementation":"handy_keys","log_level":"info","theme":"system"}}"#).unwrap();
        assert!(!handy_done(&handy_read(&store).unwrap()));

        handy_set(&store, &[("keyboard_implementation", "tauri"), ("log_level", "debug")]).unwrap();

        let settings = handy_read(&store).unwrap();
        assert!(handy_done(&settings));
        assert_eq!(settings["settings"]["theme"], "system");
    }

    #[test]
    fn fluxer_gets_push_to_mute_next_to_existing_keybinds() {
        let store = scratch("fluxer").join("desktop-app-store.sqlite3");
        let state = r#"{"customKeybinds":[{"id":"a","action":"voice_toggle_mute","combo":{"key":"m","code":"KeyM","alt":true,"global":true,"enabled":true},"enabled":true}],"transmitMode":"voice_activity","__mps__":{"version":5}}"#;
        let db = Connection::open(&store).unwrap();
        db.execute_batch(
            "CREATE TABLE scoped_storage (store TEXT NOT NULL, scope TEXT NOT NULL, entry_key TEXT NOT NULL,
                value TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY(store, scope, entry_key))",
        )
        .unwrap();
        db.execute("INSERT INTO scoped_storage VALUES ('app', 'global', 'Keybind', ?1, 1.0)", [state]).unwrap();
        drop(db);
        assert!(!fluxer_done(&fluxer_read(&store).unwrap().unwrap()));

        fluxer_configure(&store).unwrap();

        let state = fluxer_read(&store).unwrap().unwrap();
        assert!(fluxer_done(&state));
        assert_eq!(state["customKeybinds"].as_array().unwrap().len(), 2);
        assert_eq!(state["__mps__"]["version"], 5);
    }

    #[test]
    fn discord_gets_push_to_mute_with_the_next_id() {
        let dir = scratch("discord").join("leveldb");
        let state = r#"{"_state":{"1":{"id":"1","enabled":true,"action":"SAVE_SCREENSHOT","shortcut":[[0,69,2]],"managed":true,"params":{}},"1004":{"id":"1004","enabled":true,"action":"SOUNDBOARD_HOLD","shortcut":[[0,224,2],[0,53,2]],"managed":true,"params":{}}},"_version":5}"#;
        let mut db = rusty_leveldb::DB::open(&dir, rusty_leveldb::Options::default()).unwrap();
        db.put(DISCORD_KEYBINDS, &local_storage_encode(state)).unwrap();
        db.close().unwrap();
        drop(db);
        assert!(!discord_done(&discord_read(&dir).unwrap().unwrap()));

        discord_configure(&dir).unwrap();

        let state = discord_read(&dir).unwrap().unwrap();
        assert!(discord_done(&state));
        assert_eq!(state["_state"]["1005"]["action"], "PUSH_TO_MUTE");
        assert_eq!(state["_state"]["1"]["action"], "SAVE_SCREENSHOT");
        assert_eq!(state["_version"], 5);
        assert!(dir.with_file_name("leveldb.duckcall-backup").join("CURRENT").exists());
    }

    #[test]
    fn local_storage_text_survives_both_encodings() {
        for text in ["plain", "latin-1 é", "utf-16 ✓"] {
            assert_eq!(local_storage_decode(&local_storage_encode(text)).as_deref(), Some(text));
        }
        assert_eq!(local_storage_encode("a"), [1, b'a']);
        assert_eq!(local_storage_encode("✓")[0], 0);
    }
}
