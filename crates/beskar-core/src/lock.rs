//! A lock that keeps two Beskar processes from changing the registry or the
//! library at the same time. Several agents working in different workspaces
//! can easily run `beskar` concurrently.
//!
//! The lock is an operating-system lock on the file `lock` in Beskar's home
//! directory, so the kernel releases it when the holding process exits, even
//! if it crashes. The file itself stays; its text only says who holds the
//! lock, for the message another process shows while it waits.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::timestamp::Timestamp;
use crate::{Error, ErrorKind, Result};

pub const LOCK_FILE: &str = "lock";

/// How long to wait quietly before telling the person why nothing happens.
const PATIENCE: Duration = Duration::from_secs(1);

/// Held until dropped.
#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Lock {
    /// Take the lock in `dir`, waiting up to `wait` for another process to
    /// release it. If the wait lasts, `waiting` hears once who holds the
    /// lock, so a front end can say why nothing happens.
    pub fn acquire(
        dir: &Path,
        command: &str,
        wait: Duration,
        waiting: &dyn Fn(&str),
    ) -> Result<Lock> {
        let path = dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| Error::io(&err, format_args!("open lock file {}", path.display())))?;
        let start = Instant::now();
        let mut told = false;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if start.elapsed() < wait => {
                    if !told && start.elapsed() >= PATIENCE {
                        told = true;
                        waiting(&describe(&fs::read_to_string(&path).unwrap_or_default()));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(TryLockError::WouldBlock) => {
                    let holder = describe(&fs::read_to_string(&path).unwrap_or_default());
                    return Err(Error::new(
                        ErrorKind::Locked,
                        format!(
                            "another beskar process is busy ({holder}); gave up after {}",
                            seconds(wait)
                        ),
                    )
                    .hint("run the command again once it has finished")
                    .hint("to wait longer, set `lock-timeout: <seconds>` in the config or BESKAR_LOCK_TIMEOUT"));
                }
                Err(TryLockError::Error(err)) => {
                    return Err(Error::io(&err, format_args!("lock {}", path.display())));
                }
            }
        }
        let command: String = command.chars().filter(|c| !c.is_control()).collect();
        let note = format!(
            "pid: {}\nsince: {}\ncommand: {}\n",
            std::process::id(),
            Timestamp::now(),
            command.trim()
        );
        let _ = file
            .set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)))
            .and_then(|_| file.write_all(note.as_bytes()));
        Ok(Lock { file })
    }

    /// Who holds the lock in `dir` right now, if anyone does.
    pub fn holder(dir: &Path) -> Option<String> {
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                None
            }
            Err(TryLockError::WouldBlock) => {
                Some(describe(&fs::read_to_string(&path).unwrap_or_default()))
            }
            Err(TryLockError::Error(_)) => None,
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn seconds(wait: Duration) -> String {
    match wait.as_secs() {
        1 => "1 second".to_string(),
        n => format!("{n} seconds"),
    }
}

/// `pid: 42` lines as `pid 42, since ..., command ...`.
fn describe(text: &str) -> String {
    let parts: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| format!("{key} {value}"))
        .collect();
    if parts.is_empty() {
        "no details".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn a_second_acquire_waits_and_fails() {
        let tmp = TempDir::new();
        let lock = Lock::acquire(tmp.path(), "repo update", Duration::ZERO, &|_| {}).unwrap();
        let error = Lock::acquire(
            tmp.path(),
            "repo update",
            Duration::from_millis(120),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Locked);
        assert!(
            error
                .message
                .contains(&format!("pid {}", std::process::id())),
            "{}",
            error.message
        );
        assert!(Lock::holder(tmp.path()).is_some());
        drop(lock);
        assert_eq!(Lock::holder(tmp.path()), None);
        Lock::acquire(tmp.path(), "repo update", Duration::ZERO, &|_| {}).unwrap();
    }

    #[test]
    fn a_leftover_lock_file_is_not_a_lock() {
        let tmp = TempDir::new();
        std::fs::write(
            tmp.path().join(LOCK_FILE),
            "pid: 4294967\nsince: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        assert_eq!(Lock::holder(tmp.path()), None);
        Lock::acquire(tmp.path(), "test", Duration::ZERO, &|_| {}).unwrap();
    }

    #[test]
    fn waiting_ends_when_the_holder_lets_go() {
        let tmp = TempDir::new();
        let lock = Lock::acquire(tmp.path(), "first", Duration::ZERO, &|_| {}).unwrap();
        let dir = tmp.path().to_path_buf();
        let waiter = thread::spawn(move || {
            let told = std::sync::Mutex::new(Vec::new());
            Lock::acquire(&dir, "second", Duration::from_secs(10), &|holder| {
                told.lock().unwrap().push(holder.to_string())
            })
            .map(|_| told.into_inner().unwrap())
        });
        thread::sleep(PATIENCE + Duration::from_millis(300));
        drop(lock);
        let told = waiter.join().unwrap().unwrap();
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].contains("command first"), "{told:?}");
    }
}
