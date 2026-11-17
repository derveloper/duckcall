//! duckcall: mutes you in the call and ducks the output volume while Handy records a dictation.
//!
//! The mute is the call app's own "push to mute" on Handy's shortcut; `install` sets that up.
//! The ducking is `run`, a background service that follows Handy's log.

mod apps;
mod duck;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
compile_error!("duckcall supports only macOS so far");

use apps::{App, Setup};
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

const USAGE: &str = "usage: duckcall <command>
  install [--yes]     set up Handy and the call apps, install Duckcall.app and start its background service
  uninstall [--yes]   remove Duckcall.app and its service, restore Handy's keyboard implementation
  run                 follow Handy and duck the volume (what the background service runs)
  register            switch the background service on (what Duckcall.app does when it is opened)
  unregister          switch the background service off

--yes restarts running apps without asking.
DUCK_VOLUME=10 duckcall install                         ducks to another output volume (default 5, range 0-100)
DUCKCALL_APPLICATIONS=~/Applications duckcall install   puts the app there instead of /Applications";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let complete = match args.as_slice() {
        ["install"] => install(false),
        ["install", "--yes"] => install(true),
        ["uninstall"] => uninstall(false),
        ["uninstall", "--yes"] => uninstall(true),
        ["run"] => duck::run().map(|()| true),
        ["register"] => platform::app_service(true).map(|()| true),
        ["unregister"] => platform::app_service(false).map(|()| true),
        // Duckcall.app opened like any other app: it switches its background service on.
        [] if platform::in_app() => platform::app_service(true).map(|()| true),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match complete {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("duckcall: {error}");
            ExitCode::FAILURE
        }
    }
}

fn install(yes: bool) -> Result<bool> {
    let level = duck::level()?;
    let shortcut = apps::handy_get("/settings/bindings/transcribe/current_binding")?;
    // ponytail: only Handy's default shortcut is translated into the call apps' key formats;
    // add per-app key tables when another shortcut has to work.
    let apps: &[App] = if shortcut == "option+space" {
        &App::ALL
    } else {
        println!("Handy's shortcut is {shortcut}, not option+space: bind \"push to mute\" to it by hand in the call app");
        &[App::Handy]
    };
    let mut pending = Vec::new();
    for &app in apps {
        match app.setup()? {
            Setup::Done => println!("{}: already set up", app.name()),
            Setup::Pending => pending.push(app),
            Setup::Absent => println!("{}: not found; if you use it, start it once and run this again", app.name()),
        }
    }
    let complete = apply(&pending, yes, App::configure)?;
    platform::service_install(level)?;
    println!("Duckcall.app runs in the background and ducks to volume {level}");
    Ok(complete)
}

fn uninstall(yes: bool) -> Result<bool> {
    platform::service_remove()?;
    println!("Duckcall.app and its background service removed");
    let store = platform::store(App::Handy);
    let changed = store.exists() && apps::handy_get("/settings/keyboard_implementation")? != platform::HANDY_KEYBOARD;
    let handy: &[App] = if changed { &[App::Handy] } else { &[] };
    let complete = apply(handy, yes, |_| apps::handy_set(&store, &[("keyboard_implementation", platform::HANDY_KEYBOARD)]))?;
    println!("The push-to-mute shortcuts in Fluxer and Discord stay; remove them there if unwanted");
    Ok(complete)
}

/// Applies `change` to each app. Running apps are restarted around it, if the user agrees.
/// Returns whether every app was changed.
fn apply(apps: &[App], yes: bool, change: impl Fn(App) -> Result<()>) -> Result<bool> {
    let running: Vec<App> = apps.iter().copied().filter(|&app| platform::is_running(app)).collect();
    let restart = running.is_empty() || yes || {
        let names: Vec<&str> = running.iter().map(|app| app.name()).collect();
        confirm(&format!("{} must be restarted for this (a restart ends active calls). Restart now?", names.join(" and ")))
    };
    let mut complete = true;
    for &app in apps {
        let was_running = running.contains(&app);
        if was_running && !restart {
            println!("{}: skipped because it is running; quit it and run this again", app.name());
            complete = false;
            continue;
        }
        if was_running {
            platform::stop(app)?;
        }
        let changed = change(app);
        if was_running {
            platform::start(app)?;
        }
        changed?;
        println!("{}: settings changed", app.name());
    }
    Ok(complete)
}

/// Asks on the terminal; without one the answer is no.
fn confirm(question: &str) -> bool {
    if !io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [y/N] ");
    io::stdout().flush().ok();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes" | "j" | "J" | "ja")
}
