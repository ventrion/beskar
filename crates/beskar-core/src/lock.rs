//! Advisory file-lock acquisition (spec §29, §88).
//!
//! Registry writes and per-target mutations take exclusive advisory locks.
//! Acquisition retries briefly before failing closed: a lock that was just
//! released can transiently still read as held while concurrent fork/exec
//! activity (Beskar shells out to system Git, §68) settles, and a spurious
//! failure there would abort valid work. Genuine contention — another
//! Beskar process holding the resource — still fails closed with a typed
//! error naming the resource and the recovery guidance (§88). Locks are
//! never removed blindly.

use std::path::Path;
use std::time::{Duration, Instant};

/// How long acquisition keeps retrying before reporting contention. Long
/// enough to ride out transient release lag; short enough that a genuinely
/// stuck peer surfaces as a typed [`crate::Error::Lock`] quickly.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

const RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// Creates (or truncates) the advisory lock file at `path` and acquires an
/// exclusive lock on it, returning the guard that holds the lock. The lock
/// releases when the returned [`std::fs::File`] drops.
///
/// `what` names the resource class for error messages (§88: identify the
/// locked resource and give recovery guidance).
pub(crate) fn lock_file_exclusive(path: &Path, what: &str) -> crate::Result<std::fs::File> {
    // Ensure the parent directory exists so first use in a fresh home works.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(crate::Error::lock(format!(
                        "{what} is locked by another process: {path:?} — \
                         another Beskar command may be running; wait for it \
                         to finish instead of deleting the lock (§88) [{err}]"
                    )));
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}
