# AGENTS.md

Notes for LLM agents working on duckcall.

## What this is

A small Rust CLI for one workflow: the user dictates to an LLM with [Handy](https://github.com/cjpais/Handy)
while sitting in a voice call (Fluxer or Discord). While Handy records,

- the call app mutes the microphone itself, through its "push to mute" shortcut on Handy's key (Option+Space),
- `duckcall run` lowers the system output volume, so the call stays out of the transcript, and restores it afterwards.

macOS only so far. Linux is planned: keep everything macOS-specific in `src/macos.rs`.

## How it works

- **Trigger.** `run` follows Handy's log (`~/Library/Logs/com.pais.handy/handy.log`) and reacts to
  `TranscribeAction::start called`, `TranscribeAction::stop called` and `Operation cancellation completed`.
  These are DEBUG lines, so Handy's `log_level` must be `debug`.
- **Mute.** duckcall mutes nothing. `install` writes a push-to-mute keybind into the call app's settings store.
- **Why Handy is switched to `keyboard_implementation = tauri`.** Handy's macOS default, `handy_keys`, runs an
  active event tap and deletes the shortcut's key event. Fluxer only listens and never sees the key. With `tauri`,
  Handy registers a system hotkey and the event stays visible to other listeners.
- **Service.** A LaunchAgent (`io.github.lansidev.duckcall`) runs `duckcall run` from an app bundle that `install`
  builds around a copy of the executable: `~/Library/Application Support/duckcall/Duckcall.app`. The bundle is
  there for System Settings only. macOS names a background item after the app that contains its executable, so
  the list shows "Duckcall" with the logo instead of a bare executable. `AssociatedBundleIdentifiers`, the
  documented way, is ignored for executables without a Team ID.

## Code map

| File | Holds |
|---|---|
| `src/main.rs` | CLI, `install` / `uninstall`, consent before restarting apps (`apply`, `confirm`) |
| `src/duck.rs` | `run`: `Tail` (a polling `tail -F`) and `Duck` (remembers the volume from before) |
| `src/apps.rs` | Reading and writing the settings of Handy, Fluxer and Discord |
| `src/macos.rs` | Paths, volume (`osascript`), process control, LaunchAgent and app bundle; imported as `platform` |
| `tests/cli.rs` | End-to-end tests: the real binary in a sandbox (`Sandbox`) |
| `assets/logo.svg` | The logo, shown in the README |
| `assets/duckcall.icns` | The app bundle's icon, compiled into the binary. After changing the logo, run `assets/icns.sh` |

A Linux port adds `src/linux.rs` with the same public items as `macos.rs` and extends the `cfg` in `main.rs`.

## Rules

- **Never write an app's store while the app is running.** It would not pick the change up and may overwrite it.
  Stop the app, write, start it again; `apply` in `main.rs` does this.
- **Never restart an app without consent.** A restart ends the user's active call. `confirm` asks, `--yes` skips
  the question, and without a terminal the app is skipped and the exit code is 1.
- **Never open Discord's LevelDB in place just to read it.** Opening replays the log and rewrites the manifest.
  `discord_read` works on a copy. Before writing, `discord_configure` copies the directory to
  `leveldb.duckcall-backup`, because that LevelDB also holds the Discord login.
- **Do not test against the real apps or stores.** Unit tests build their own stores, and `tests/cli.rs` runs the
  binary in a sandbox: `HOME` is a scratch directory and the only `PATH` entry holds stand-ins for `launchctl`,
  `pgrep`, `pkill`, `open`, `osascript` and `codesign`. If you copy a real Fluxer or Discord store for an experiment, it
  contains login tokens: delete it afterwards and keep it out of the repository.
- **Assume the user is in a live call.** Do not play sound, do not restart Handy, Fluxer or Discord, and change
  the real output volume only for a short check that you announce.
- **Keep the simplifications marked.** Comments starting with `ponytail:` name a deliberate shortcut and its
  upgrade path (system-wide ducking, polling, only `option+space`). Update the comment when you lift one.
- **Keep dependencies few.** `libc`, `rusqlite` (bundled SQLite), `rusty-leveldb`, `serde_json`; the command line
  is parsed by hand.

## The settings stores

None of these formats is documented; they were read from the apps' data and client code. Preserve every field
you do not understand, and fail with a message instead of guessing when something looks different.

| App | Store | Entry |
|---|---|---|
| Handy | `~/Library/Application Support/com.pais.handy/settings_store.json` | `settings.keyboard_implementation`, `settings.log_level`; the shortcut is `settings.bindings.transcribe.current_binding` |
| Fluxer | `~/Library/Application Support/fluxer/desktop-app-store.sqlite3`, table `scoped_storage` | row `store='app'`, `scope='global'`, `entry_key='Keybind'`: JSON with `customKeybinds` |
| Discord | `~/Library/Application Support/discord/Local Storage/leveldb` (Chromium localStorage) | key `_https://discord.com\x00\x01keybinds`: JSON with `_state` |

- Fluxer persists its row with mobx-persist-store and discards it when `__mps__.version` is not the version the
  client expects. So duckcall only extends an existing row and never creates one.
- Discord values start with a marker byte: `0` = UTF-16LE, `1` = Latin-1. Keybind ids must be numeric strings equal
  to their key in `_state`; a new one is the highest id + 1. A shortcut is a list of
  `[device, key code, environment]`: on macOS the key codes are USB HID usages and the environment is `2`
  (`DISCORD_OPTION_SPACE` in `macos.rs`). Other systems use other key codes and another environment value.
- Push to mute only takes effect in voice-activity mode, in both apps.
- Only Handy's default shortcut `option+space` is translated into the call apps' formats.

## Tests

```sh
cargo test                  # everything below; the real apps, service and volume stay untouched
cargo clippy --all-targets  # must stay free of warnings
```

Every change comes with a test for the behaviour it changes.

| Where | Covers |
|---|---|
| `src/duck.rs` | `DUCK_VOLUME` parsing, the ducking rules (`Duck`), following a log (`Tail`) |
| `src/apps.rs` | Per app: what counts as set up, what gets written, what is refused and left as it was |
| `src/macos.rs` | The LaunchAgent is a plist that `plutil` can read |
| `tests/cli.rs` | `install`, `uninstall` and `run` end to end, through the real binary in the sandbox |

- For new end-to-end behaviour, add a fixture or a stand-in to `Sandbox`. A stand-in must never fall through to
  the real tool; that is why the sandbox `PATH` contains nothing else.
- Tests of `run` never sleep and hope. They write a log line until the service has reacted (`until`); the two
  `run_…` tests show the pattern.
- Test names say what must hold (`install_never_restarts_a_running_app_without_consent`). Assert effects (stores,
  calls of the stand-ins, exit codes) rather than the wording of messages.
- After changing the LevelDB code, also modify a copy of a real Discord store and read it with a second LevelDB
  implementation (for example Node's `classic-level`): every key must still be there, with only `keybinds` changed.
- The service runs its own copy of the binary, inside the app bundle. To try a change for real:
  `cargo install --path . --root ~/.local`, then `duckcall install`, which replaces the copy.
- To see what macOS calls the background item without opening System Settings, look at `name=` right after
  `duckcall install`:
  `log show --last 1m --info --predicate 'process == "backgroundtaskmanagementd"' | grep lansidev`
- `HANDY_LOG=<file> duckcall run` follows another file; appending the trigger lines to it exercises `run`
  without Handy. Outside the sandbox this changes the real output volume.

## Conventions

- Documentation, comments and command-line output are in English.
- Commit messages follow Conventional Commits in the imperative (`feat: add …`, `fix(discord): …`).
