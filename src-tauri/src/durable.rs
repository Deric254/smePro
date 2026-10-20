//! Crash-safe reads and writes for the small files that live outside the
//! database (the licence file, the customer list, the database key).
//!
//! The database gets this from SQLite. These files had a plain
//! `write`-then-`rename`, which is atomic against a crash of the app but NOT
//! against a power cut: without `sync_all` the OS may rename the file before
//! its bytes reach the disk, leaving a zero-length file after the restart.
//!
//! `write_atomic` writes a temp file, forces it to disk, renames it over the
//! real file, then forces the folder entry to disk. `read_text` tells "the
//! file isn't there" apart from "the file couldn't be read right now" (antivirus
//! or a sync client holding it for a moment), because treating the second as
//! the first is how a licence used to get overwritten with an empty one.

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

/// Antivirus and sync clients hold files for a few hundred milliseconds at most.
const ATTEMPTS: u32 = 6;
const PAUSE: Duration = Duration::from_millis(60);

/// `name.ext` -> `name.ext<suffix>` in the same folder (`licensing.json.bak`).
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// `Ok(None)` when the file does not exist; `Err` for any other failure, after a few retries.
pub fn read_text(path: &Path) -> std::io::Result<Option<String>> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match fs::read_to_string(path) {
            Ok(t) => return Ok(Some(t)),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            // Not valid UTF-8 means the content is damaged, not that the file is busy: no point retrying.
            Err(e) if e.kind() == ErrorKind::InvalidData => return Err(e),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < ATTEMPTS {
                    sleep(PAUSE);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("unreadable")))
}

/// Replace `path` with `bytes`, all or nothing, and on disk before returning.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, ".tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match fs::rename(&tmp, path) {
            Ok(()) => {
                sync_dir(path);
                return Ok(());
            }
            Err(e) => {
                last = Some(e);
                if attempt + 1 < ATTEMPTS {
                    sleep(PAUSE);
                }
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Make the rename itself survive a power cut. Folders can't be opened for this on Windows.
#[cfg(unix)]
fn sync_dir(path: &Path) {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("smp-durable-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("f.json")
    }

    #[test]
    fn write_then_read_round_trips_and_leaves_no_temp_file() {
        let p = scratch("rt");
        assert_eq!(read_text(&p).unwrap(), None, "a missing file is None, not an error");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(read_text(&p).unwrap().as_deref(), Some("two"));
        assert!(!sibling(&p, ".tmp").exists());
    }

    #[test]
    fn an_unreadable_file_is_an_error_never_a_missing_one() {
        let p = scratch("dir");
        fs::create_dir_all(&p).unwrap(); // a folder where the file should be
        assert!(read_text(&p).is_err());
    }

    #[test]
    fn sibling_appends_to_the_whole_file_name() {
        assert_eq!(sibling(Path::new("/a/erp.db.key"), ".tmp"), PathBuf::from("/a/erp.db.key.tmp"));
    }
}
