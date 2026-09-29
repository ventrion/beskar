//! Filesystem operations with explicit boundaries and no symlink traversal.
use crate::model::path_text;
use crate::sha256::Sha256;
use crate::{Error, Result, fail};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.try_exists()? {
        let result = path.canonicalize()?;
        path_text(&result)?;
        return Ok(result);
    }
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut ancestor = normalized.as_path();
    let mut tail = Vec::new();
    while !ancestor.try_exists()? {
        tail.push(
            ancestor
                .file_name()
                .ok_or_else(|| Error("cannot resolve path".into()))?,
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| Error("cannot resolve path".into()))?;
    }
    let mut result = ancestor.canonicalize()?;
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    path_text(&result)?;
    Ok(result)
}

/// Inspect the path itself without following a final symlink; absence is `None`.
pub fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => fail(format!("{}: {e}", path.display())),
    }
}

pub fn reject_links(path: &Path) -> Result<()> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return fail("parent traversal is not allowed");
        }
        prefix.push(part.as_os_str());
        if let Some(m) = metadata(&prefix)?
            && m.file_type().is_symlink()
        {
            return fail(format!("symlink is not allowed: {}", prefix.display()));
        }
    }
    Ok(())
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    reject_links(path)?;
    fs::create_dir_all(path)
        .map_err(|e| Error(format!("create directory {}: {e}", path.display())))?;
    Ok(())
}

pub fn require_dir(path: &Path) -> Result<()> {
    reject_links(path)?;
    if !metadata(path)?.is_some_and(|m| m.is_dir()) {
        return fail(format!("directory is missing: {}", path.display()));
    }
    Ok(())
}

pub fn read_text(path: &Path) -> Result<String> {
    reject_links(path)?;
    if !metadata(path)?.is_some_and(|m| m.is_file()) {
        return fail(format!("file is missing: {}", path.display()));
    }
    fs::read_to_string(path).map_err(|e| Error(format!("read {}: {e}", path.display())))
}

pub fn children(path: &Path) -> Result<Vec<PathBuf>> {
    require_dir(path)?;
    let mut entries: Vec<_> = fs::read_dir(path)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    entries.sort();
    Ok(entries)
}

fn token() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn temp_dir(parent: &Path, purpose: &str) -> Result<PathBuf> {
    ensure_dir(parent)?;
    let path = parent.join(format!(".beskar-{purpose}-{}", token()));
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    reject_links(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error("file has no parent".into()))?;
    require_dir(parent)?;
    let temp = parent.join(format!(".beskar-write-{}", token()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e: Error| Error(format!("write {}: {e}", path.display())))
}

pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(path: &Path, create: bool) -> Result<Self> {
        reject_links(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)
            .map_err(|e| Error(format!("open lock {}: {e}", path.display())))?;
        file.try_lock().map_err(|e| {
            Error(format!(
                "Beskar state is busy or cannot be locked at {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self { _file: file })
    }
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111
}
#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> u32 {
    0
}

fn frame(hash: &mut Sha256, value: &[u8]) {
    hash.update(&(value.len() as u64).to_be_bytes());
    hash.update(value);
}

pub fn fingerprint(path: &Path) -> Result<String> {
    require_dir(path)?;
    let mut hash = Sha256::new();
    hash.update(b"beskar-tree-v1\0");
    hash_tree(path, path, &mut hash)?;
    Ok(hash.finish())
}

fn hash_tree(root: &Path, path: &Path, hash: &mut Sha256) -> Result<()> {
    for child in children(path)? {
        let m = fs::symlink_metadata(&child)?;
        let relative = child.strip_prefix(root).unwrap();
        let mut parts = Vec::new();
        for part in relative.components() {
            parts.push(part.as_os_str().to_str().ok_or_else(|| {
                Error(format!(
                    "skill filenames must be UTF-8: {}",
                    child.display()
                ))
            })?);
        }
        let name = parts.join("/");
        if m.is_dir() {
            hash.update(b"d");
            frame(hash, name.as_bytes());
            hash_tree(root, &child, hash)?;
        } else if m.is_file() {
            hash.update(b"f");
            frame(hash, name.as_bytes());
            hash.update(&executable(&m).to_be_bytes());
            hash.update(&m.len().to_be_bytes());
            let mut file = File::open(&child)?;
            let mut buffer = [0u8; 65536];
            let mut bytes = 0u64;
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                hash.update(&buffer[..n]);
            }
            let after = file.metadata()?;
            if bytes != m.len()
                || after.len() != m.len()
                || after.modified().ok() != m.modified().ok()
            {
                return fail(format!("file changed while reading: {}", child.display()));
            }
        } else {
            return fail(format!(
                "only ordinary files and directories are supported; symlink or special file: {}",
                child.display()
            ));
        }
    }
    Ok(())
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    require_dir(source)?;
    reject_links(destination)?;
    fs::create_dir(destination)?;
    copy_contents(source, destination)
}

fn copy_contents(source: &Path, destination: &Path) -> Result<()> {
    for child in children(source)? {
        let target = destination.join(child.file_name().unwrap());
        let m = fs::symlink_metadata(&child)?;
        if m.is_dir() {
            fs::create_dir(&target)?;
            copy_contents(&child, &target)?;
        } else if m.is_file() {
            fs::copy(&child, &target)?;
            fs::set_permissions(&target, m.permissions())?;
        } else {
            return fail(format!(
                "cannot copy symlink or special file: {}",
                child.display()
            ));
        }
    }
    Ok(())
}

pub fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
