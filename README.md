<p align="center">
  <img src="assets/logo.svg" width="180" alt="duckcall logo: a duck wearing headphones">
</p>

# duckcall

Dictate without the call talking over you. While [Handy](https://github.com/cjpais/Handy) records (hold ⌥Space),

- the call app (Fluxer or Discord) mutes your microphone through its own "push to mute" on the same key,
- duckcall lowers the system volume to 5 and restores it afterwards.

macOS only so far; Linux is planned.

## Installation

```sh
git clone https://github.com/lansidev/duckcall
cargo install --path duckcall --root ~/.local
duckcall install
```

`--root ~/.local` puts the binary into `~/.local/bin`. `duckcall install` then sets everything up:

| What | Where | Why |
|---|---|---|
| Handy: `keyboard_implementation = tauri` | `settings_store.json` | Handy's default (`handy_keys`) swallows the key press, so Fluxer would never see it. |
| Handy: `log_level = debug` | `settings_store.json` | duckcall detects the start and end of a recording from Handy's DEBUG log lines. |
| Fluxer: "Push to mute" on ⌥Space | `desktop-app-store.sqlite3` | Mutes the microphone while the key is held. |
| Discord: "Push to Mute" on ⌥Space | localStorage (LevelDB) | Same as Fluxer. The previous state is copied to `leveldb.duckcall-backup` first. |
| Background service | LaunchAgent `io.github.lansidev.duckcall`, app bundle `~/Library/Application Support/duckcall/Duckcall.app` | Starts `duckcall run` at login. It runs from an app bundle so that System Settings lists the background item as "Duckcall" with its icon. |

Settings that already match are left alone. If an app whose settings have to change is running,
duckcall asks before restarting it, because a restart ends active calls. `--yes` restarts without
asking; without a terminal and without `--yes` the app is skipped.

Requirements:

- Handy, Fluxer and Discord must each have been started once. A call app that is missing is skipped.
- Fluxer and Discord need the macOS "Input Monitoring" permission; without it they only see keys in their own window.
- In both apps "push to mute" only works in voice-activity mode, not with push-to-talk.

## Configure and remove

```sh
DUCK_VOLUME=10 duckcall install   # another volume while dictating (0-100, default 5)
duckcall uninstall                # remove the service, set Handy back to handy_keys
```

The push-to-mute shortcuts in Fluxer and Discord stay after `uninstall`.

After updating duckcall, run `duckcall install` again: the service runs its own copy of the binary.

## Limits

- The whole system output is ducked, not only the call app. Per-app volume on macOS would need a
  Core Audio process tap with the audio-capture permission.
- Outputs without a volume control (HDMI) are left unchanged.
- Only Handy's default shortcut `option+space` is set up automatically. With another shortcut,
  `install` only changes Handy; you have to bind "push to mute" to it yourself.
- Fluxer and Discord do not document their settings stores. If an update changes the format,
  `install` reports an error or the app ignores the entry.
- If Handy changes the wording of its log lines (`TranscribeAction::start called` / `stop called`),
  the ducking stops working.

## Development

```sh
cargo test                  # unit tests and end-to-end tests in a sandbox; the real apps and volume stay untouched
cargo clippy --all-targets
```

[AGENTS.md](AGENTS.md) explains how the pieces fit together and what to watch out for.
