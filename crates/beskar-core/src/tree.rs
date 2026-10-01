use crate::{Result, format, io, sha256::Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn unique() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Reject symlinks in every existing component, including dangling links.
pub fn safe_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(format!(
                "{}: parent traversal is not allowed",
                path.display()
            ));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("{}: symlinks are not supported", current.display()));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("{}: {e}", current.display())),
        }
    }
    Ok(())
}

pub fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn children(path: &Path) -> Result<Vec<PathBuf>> {
    safe_path(path)?;
    let mut result = io(path.display(), fs::read_dir(path))?
        .map(|item| item.map(|v| v.path()))
        .collect::<std::io::Result<Vec<_>>>();
    let paths = result
        .as_mut()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    paths.sort();
    Ok(std::mem::take(paths))
}

fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn executable(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0
    }
}

fn walk_hash(root: &Path, path: &Path, hash: &mut Sha256) -> Result<()> {
    let meta = io(path.display(), fs::symlink_metadata(path))?;
    let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
    // Normalize separators to keep the format stable across hosts.
    let name = relative
        .components()
        .map(|c| format::path_text(Path::new(c.as_os_str())))
        .collect::<Result<Vec<_>>>()?
        .join("/");
    feed(hash, name.as_bytes());
    if meta.is_dir() {
        hash.update(b"D");
        for child in children(path)? {
            walk_hash(root, &child, hash)?;
        }
    } else if meta.is_file() {
        hash.update(b"F");
        hash.update(&executable(&meta).to_be_bytes());
        hash.update(&meta.len().to_be_bytes());
        let mut file = io(path.display(), fs::File::open(path))?;
        let mut buffer = [0u8; 65536];
        let mut read = 0u64;
        loop {
            let count = io(path.display(), file.read(&mut buffer))?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            read += count as u64;
        }
        if read != meta.len() {
            return Err(format!("{} changed while reading; retry", path.display()));
        }
    } else {
        return Err(format!(
            "{}: only regular files and directories are supported",
            path.display()
        ));
    }
    Ok(())
}

pub fn fingerprint(path: &Path) -> Result<String> {
    safe_path(path)?;
    let mut hash = Sha256::default();
    hash.update(b"beskar-tree-v1\0");
    walk_hash(path, path, &mut hash)?;
    Ok(hash.finish())
}

/// The tree fingerprint of a newly written, non-executable text file.
pub fn text_fingerprint(text: &str) -> String {
    let mut hash = Sha256::default();
    hash.update(b"beskar-tree-v1\0");
    feed(&mut hash, b"");
    hash.update(b"F");
    hash.update(&0u32.to_be_bytes());
    hash.update(&(text.len() as u64).to_be_bytes());
    hash.update(text.as_bytes());
    hash.finish()
}

pub fn optional_hash(path: &Path) -> Result<Option<String>> {
    if exists(path)? {
        Ok(Some(fingerprint(path)?))
    } else {
        Ok(None)
    }
}

pub fn copy(source: &Path, target: &Path) -> Result<()> {
    safe_path(source)?;
    safe_path(target)?;
    let meta = io(source.display(), fs::symlink_metadata(source))?;
    if meta.is_dir() {
        io(target.display(), fs::create_dir(target))?;
        for child in children(source)? {
            copy(
                &child,
                &target.join(child.file_name().ok_or("missing file name")?),
            )?;
        }
        // Files retain all permissions; directory access stays usable for future updates.
        sync_dir(target)?;
    } else if meta.is_file() {
        // Write through one handle: Windows flushes only handles with write access,
        // and a copied read-only attribute would block a second, writable open.
        let mut input = io(source.display(), fs::File::open(source))?;
        let mut output = io(
            target.display(),
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target),
        )?;
        io(target.display(), std::io::copy(&mut input, &mut output))?;
        io(target.display(), output.set_permissions(meta.permissions()))?;
        io(target.display(), output.sync_all())?;
    } else {
        return Err(format!(
            "{}: only regular files and directories are supported",
            source.display()
        ));
    }
    Ok(())
}

pub fn remove(path: &Path) -> Result<()> {
    safe_path(path)?;
    if !exists(path)? {
        return Ok(());
    }
    let meta = io(path.display(), fs::symlink_metadata(path))?;
    if meta.is_dir() {
        io(path.display(), fs::remove_dir_all(path))
    } else {
        io(path.display(), fs::remove_file(path))
    }
}

pub fn write_new(path: &Path, data: &str) -> Result<()> {
    safe_path(path)?;
    let mut file = io(
        path.display(),
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path),
    )?;
    io(path.display(), file.write_all(data.as_bytes()))?;
    io(path.display(), file.sync_all())
}

pub fn atomic_write(path: &Path, data: &str) -> Result<()> {
    safe_path(path)?;
    let parent = path.parent().ok_or("file needs a parent directory")?;
    let temporary = parent.join(format!(".beskar-write-{}", unique()));
    let result = (|| {
        write_new(&temporary, data)?;
        io(path.display(), fs::rename(&temporary, path))?;
        sync_dir(parent)
    })();
    if exists(&temporary)? {
        let _ = remove(&temporary);
    }
    result
}

/// Persist directory entries on platforms that support opening directories.
pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let directory = io(path.display(), fs::File::open(path))?;
        io(path.display(), directory.sync_all())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Normalize a user-supplied root and resolve symlinks in the part that exists.
///
/// System directories such as macOS `/var` may be symlinks. Resolving them once
/// here lets `safe_path` reject every symlink below the roots Beskar manages.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err("path cannot be empty".into());
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        io("current directory", std::env::current_dir())?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => (),
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component),
        }
    }
    let resolved = resolve(&normalized)?;
    safe_path(&resolved)?;
    format::path_text(&resolved)?;
    Ok(resolved)
}

/// Canonicalize the longest existing prefix and append the missing remainder.
/// A dangling symlink counts as missing, so `safe_path` still rejects it.
fn resolve(path: &Path) -> Result<PathBuf> {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        match fs::canonicalize(existing) {
            Ok(real) => {
                let mut real = familiar(real);
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    existing
                        .file_name()
                        .ok_or_else(|| format!("{}: no existing ancestor", path.display()))?,
                );
                existing = existing.parent().ok_or("path needs a parent")?;
            }
            Err(e) => return Err(format!("{}: {e}", existing.display())),
        }
    }
}

/// Windows canonicalization returns verbatim `\\?\` paths. Convert them to the
/// usual form when it names the same file, so they match paths from the shell.
#[cfg(windows)]
fn familiar(path: PathBuf) -> PathBuf {
    use std::path::Prefix;
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let base = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => format!("{}:", char::from(drive)),
        Prefix::VerbatimUNC(server, share) => match (server.to_str(), share.to_str()) {
            (Some(server), Some(share)) => format!(r"\\{server}\{share}"),
            _ => return path,
        },
        _ => return path,
    };
    let rest = components.as_path();
    // Win32 parsing would strip trailing dots and spaces or map device names.
    let plain = rest.components().all(|c| match c {
        Component::Normal(name) => name
            .to_str()
            .is_some_and(|name| !name.ends_with(['.', ' ']) && !device_name(name)),
        _ => true,
    });
    if plain {
        PathBuf::from(base).join(rest)
    } else {
        path
    }
}

#[cfg(windows)]
fn device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let numbered = |prefix: &str| {
        stem.len() == 4
            && stem
                .get(..3)
                .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
            && stem.as_bytes()[3].is_ascii_digit()
    };
    ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|d| stem.eq_ignore_ascii_case(d))
        || numbered("COM")
        || numbered("LPT")
}

#[cfg(not(windows))]
fn familiar(path: PathBuf) -> PathBuf {
    path
}
