//! `duckcall run`: follows Handy's log and ducks the output volume while Handy records.

use crate::{Result, platform};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Output volume while Handy records, 0-100.
pub fn level() -> Result<u8> {
    match std::env::var("DUCK_VOLUME") {
        Err(_) => Ok(5),
        Ok(level) => level
            .parse()
            .ok()
            .filter(|level| *level <= 100)
            .ok_or_else(|| "DUCK_VOLUME must be a number from 0 to 100".into()),
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_signal: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

pub fn run() -> Result<()> {
    let level = level()?;
    // The service may be stopped in the middle of a dictation; the volume must not stay ducked.
    unsafe {
        let handler = stop as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
    let mut duck = Duck::default();
    let mut tail = Tail::new(platform::handy_log());
    while !STOP.load(Ordering::Relaxed) {
        match tail.next() {
            Some(line) => duck.line(&line, || platform::duck_volume(level), platform::set_volume),
            // ponytail: polls; kqueue/inotify if 20 wakeups a second ever matter.
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    if let Some(volume) = duck.before {
        platform::set_volume(volume);
    }
    Ok(())
}

/// Holds the volume from before ducking, so repeated starts and stops change nothing.
#[derive(Default)]
struct Duck {
    before: Option<u8>,
}

impl Duck {
    // Handy's own log is the trigger, so every Handy shortcut and activation mode is covered.
    // These are DEBUG lines: Handy's log_level must stay "debug" (install sets it).
    fn line(&mut self, line: &str, duck: impl FnOnce() -> Option<u8>, restore: impl FnOnce(u8)) {
        if line.contains("TranscribeAction::start called") {
            if self.before.is_none() {
                self.before = duck();
            }
        } else if (line.contains("TranscribeAction::stop called") || line.contains("Operation cancellation completed"))
            && let Some(volume) = self.before.take()
        {
            restore(volume);
        }
    }
}

/// `tail -F`: yields the lines appended to a file, across truncation and rotation.
struct Tail {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    line: Vec<u8>,
}

impl Tail {
    /// Starts at the current end of the file; a file that appears later is read from its start.
    fn new(path: PathBuf) -> Self {
        let reader = File::open(&path).ok().and_then(|mut file| {
            file.seek(SeekFrom::End(0)).ok()?;
            Some(BufReader::new(file))
        });
        Tail { path, reader, line: Vec::new() }
    }

    /// The next complete line, or `None` if there is none yet.
    fn next(&mut self) -> Option<String> {
        if self.reader.is_none() {
            self.reader = File::open(&self.path).ok().map(BufReader::new);
        }
        let reader = self.reader.as_mut()?;
        if reader.read_until(b'\n', &mut self.line).unwrap_or(0) > 0 && self.line.ends_with(b"\n") {
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            return Some(line);
        }
        // At the end: an unfinished line stays buffered. Reopen if the file was replaced or truncated.
        let replaced = match (fs::metadata(&self.path), reader.get_ref().metadata()) {
            (Ok(now), Ok(open)) => now.ino() != open.ino() || now.len() < reader.stream_position().unwrap_or(0),
            _ => true,
        };
        if replaced {
            self.reader = None;
            self.line.clear();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    #[test]
    fn ducks_once_and_restores_the_volume_from_before() {
        let calls = RefCell::new(Vec::new());
        let mut duck = Duck::default();
        let mut feed = |line: &str, volume: Option<u8>| {
            duck.line(
                line,
                || {
                    calls.borrow_mut().push("duck".to_string());
                    volume
                },
                |volume| calls.borrow_mut().push(format!("restore {volume}")),
            );
        };
        feed("x TranscribeAction::start called for binding: transcribe", Some(50));
        feed("x TranscribeAction::start called for binding: transcribe", Some(5)); // must not take the ducked volume as "before"
        feed("x TranscribeAction::stop called for binding: transcribe", None);
        feed("x Operation cancellation completed - returned to idle state", None);
        feed("x TranscribeAction::start called for binding: transcribe", None); // output without volume control
        feed("x TranscribeAction::stop called for binding: transcribe", None);
        assert_eq!(*calls.borrow(), ["duck", "restore 50", "duck"]);
    }

    #[test]
    fn tail_follows_appends_unfinished_lines_and_rotation() {
        let path = std::env::temp_dir().join(format!("duckcall-tail-{}", std::process::id()));
        fs::write(&path, "old\n").unwrap();
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.next(), None);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "one\ntw").unwrap();
        assert_eq!(tail.next().as_deref(), Some("one\n"));
        assert_eq!(tail.next(), None); // "tw" is not a line yet
        writeln!(file, "o").unwrap();
        assert_eq!(tail.next().as_deref(), Some("two\n"));
        fs::remove_file(&path).unwrap(); // rotation: a new file under the same name
        fs::write(&path, "three\n").unwrap();
        assert_eq!(tail.next(), None); // notices the replacement
        assert_eq!(tail.next().as_deref(), Some("three\n"));
        fs::remove_file(&path).unwrap();
    }
}
