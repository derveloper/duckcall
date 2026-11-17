# duckcall

Diktieren, ohne dass der Call mitredet. Solange [Handy](https://github.com/cjpais/Handy) aufnimmt (⌥Space halten),

- schaltet die Call-App (Fluxer oder Discord) dein Mikro stumm, über ihr eigenes „Push to mute“ auf derselben Taste,
- senkt duckcall die Systemlautstärke auf 5 und stellt sie danach wieder her.

Bisher nur macOS, Linux ist geplant.

## Installation

```sh
git clone https://github.com/lansidev/duckcall
cargo install --path duckcall --root ~/.local
duckcall install
```

`--root ~/.local` legt das Binary nach `~/.local/bin`. `duckcall install` richtet dann alles ein:

| Was | Wo | Warum |
|---|---|---|
| Handy: `keyboard_implementation = tauri` | `settings_store.json` | Handys Standard (`handy_keys`) schluckt den Tastendruck, Fluxer sähe ihn nie. |
| Handy: `log_level = debug` | `settings_store.json` | duckcall erkennt Aufnahmestart und -ende an Handys DEBUG-Logzeilen. |
| Fluxer: „Push to mute“ auf ⌥Space | `desktop-app-store.sqlite3` | Mikro stumm, solange die Taste gehalten wird. |
| Discord: „Push to Mute“ auf ⌥Space | localStorage (LevelDB) | Wie bei Fluxer. Vorher wird der Stand nach `leveldb.duckcall-backup` kopiert. |
| Hintergrunddienst | LaunchAgent `io.github.lansidev.duckcall` | Startet `duckcall run` beim Login. |

Schon passende Einstellungen bleiben unberührt. Läuft eine App, deren Einstellungen sich ändern
müssen, fragt duckcall, ob es sie neu starten darf – das beendet laufende Calls. `--yes` startet
ohne Nachfrage neu; ohne Terminal und ohne `--yes` wird die App übersprungen.

Voraussetzungen:

- Handy, Fluxer und Discord müssen je einmal gestartet worden sein. Fehlt eine Call-App, wird sie übersprungen.
- Fluxer und Discord brauchen die macOS-Berechtigung „Eingabeüberwachung“, sonst sehen sie Tasten nur im eigenen Fenster.
- „Push to mute“ wirkt in beiden Apps nur im Modus Sprachaktivierung, nicht bei Push-to-talk.

## Einstellen und entfernen

```sh
DUCK_VOLUME=10 duckcall install   # andere Lautstärke beim Diktieren (0–100, Standard 5)
duckcall uninstall                # Dienst entfernen, Handy zurück auf handy_keys
```

Die Push-to-mute-Kürzel in Fluxer und Discord bleiben nach `uninstall` bestehen.

## Grenzen

- Gedrosselt wird die gesamte Systemausgabe, nicht nur die Call-App. Pro-App-Lautstärke bräuchte
  auf macOS einen Core-Audio-Process-Tap mit Audio-Capture-Berechtigung.
- Ausgabegeräte ohne Lautstärkeregler (HDMI) bleiben unverändert.
- Automatisch eingerichtet wird nur Handys Standardkürzel `option+space`. Bei einem anderen Kürzel
  stellt `install` nur Handy um; „Push to mute“ musst du dann selbst darauf legen.
- Fluxer und Discord dokumentieren ihre Einstellungsspeicher nicht. Ändert ein Update das Format,
  meldet `install` einen Fehler oder die App ignoriert den Eintrag.
- Ändert Handy den Wortlaut seiner Logzeilen (`TranscribeAction::start called` / `stop called`),
  greift die Drosselung nicht mehr.

## Entwicklung

```sh
cargo test
```
