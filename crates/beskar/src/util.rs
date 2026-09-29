//! Small std-only helpers: home/tilde expansion, timestamps, directory
//! walking, recursive copy, atomic writes, and display niceties.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// The user's home directory: `$HOME`, falling back to `$USERPROFILE`.
pub fn home_dir() -> Result<PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Ok(PathBuf::from(h));
    }
    if let Some(h) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
        return Ok(PathBuf::from(h));
    }
    Err(Error::msg("cannot determine home directory (set $HOME)"))
}

/// Expand a leading `~` or `~/` to the given home directory. Anything else
/// is returned unchanged. Beskar deliberately expands nothing else — no
/// `$VARS`, no globs — so a stored path always means exactly what it says.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Shorten an absolute path under the home directory to `~/...` for display.
pub fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Ok(home) = home_dir() {
        if let Ok(stripped) = path.strip_prefix(&home) {
            if !stripped.as_os_str().is_empty() {
                return format!("~/{}", stripped.display());
            }
            return "~".to_string();
        }
    }
    s.into_owned()
}

/// Current UNIX time in whole seconds.
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn iso_from_epoch(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Current time as ISO-8601 UTC.
pub fn now_iso() -> String {
    iso_from_epoch(now_epoch())
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` (the only timestamp format beskar writes).
pub fn parse_iso(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':'
        || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u64> { s[r].parse().ok() };
    let y = num(0..4)? as i64;
    let mo = num(5..7)? as u32;
    let d = num(8..10)? as u32;
    let h = num(11..13)?;
    let mi = num(14..16)?;
    let sec = num(17..19)?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    if civil_from_days(days) != (y, mo, d) {
        return None; // e.g. February 29th in a non-leap year
    }
    Some((days as u64) * 86_400 + h * 3600 + mi * 60 + sec)
}

/// Human-readable "how long ago" for a stored timestamp.
pub fn rel_time(iso: &str, now: u64) -> String {
    let Some(then) = parse_iso(iso) else {
        return iso.to_string();
    };
    let ago = now.saturating_sub(then);
    match ago {
        0..=59 => "just now".to_string(),
        a if a < 3600 => format!("{}m ago", a / 60),
        a if a < 86_400 => format!("{}h ago", a / 3600),
        a if a < 86_400 * 30 => format!("{}d ago", a / 86_400),
        _ => iso.get(..10).unwrap_or(iso).to_string(),
    }
}

/// Howard Hinnant's civil-from-days algorithm (valid for the full range
/// we care about; no date libraries in std).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Inverse of [`civil_from_days`].
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = if m > 2 { (m - 3) as u64 } else { (m + 9) as u64 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// One entry found while walking a tree, in deterministic (sorted) order.
#[derive(Debug, Clone)]
pub enum Item {
    File { rel: PathBuf },
    Symlink { rel: PathBuf, target: String },
    Dir { rel: PathBuf },
}

impl Item {
    pub fn rel(&self) -> &Path {
        match self {
            Item::File { rel } | Item::Symlink { rel, .. } | Item::Dir { rel } => rel,
        }
    }
}

/// Walk `root` recursively, collecting files and symlinks in a stable,
/// sorted order (byte order of the slash-separated relative path).
/// Symlinks are recorded but never followed, so trees cannot loop.
pub fn walk_sorted(root: &Path) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    walk_inner(root, Path::new(""), &mut out)?;
    out.sort_by_key(item_key);
    Ok(out)
}

fn item_key(i: &Item) -> String {
    i.rel().to_string_lossy().replace('\\', "/")
}

fn walk_inner(root: &Path, rel: &Path, out: &mut Vec<Item>) -> Result<()> {
    let dir = if rel.as_os_str().is_empty() { root.to_path_buf() } else { root.join(rel) };
    let rd = fs::read_dir(&dir).map_err(|e| Error::io(&dir, &e))?;
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let child_rel = rel.join(e.file_name());
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            let target = fs::read_link(e.path())
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(Item::Symlink { rel: child_rel, target });
        } else if ft.is_dir() {
            out.push(Item::Dir { rel: child_rel.clone() });
            walk_inner(root, &child_rel, out)?;
        } else {
            out.push(Item::File { rel: child_rel });
        }
    }
    Ok(())
}

/// Recursively copy `src` to `dst` (dst is created). Returns the number of
/// files copied. Existing content at `dst` must be removed by the caller.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<usize> {
    copy_inner(src, src, dst, 0)
}

fn copy_inner(src_root: &Path, src: &Path, dst_root: &Path, depth: usize) -> Result<usize> {
    if depth > 64 {
        return Err(Error::msg(format!("refusing to descend past depth 64 at {}", src.display())));
    }
    fs::create_dir_all(dst_root.join(src.strip_prefix(src_root).unwrap_or(Path::new(""))))
        .map_err(|e| Error::io(dst_root, &e))?;
    let rd = fs::read_dir(src).map_err(|e| Error::io(src, &e))?;
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    let mut count = 0;
    for e in entries {
        let target = dst_root.join(e.path().strip_prefix(src_root).unwrap_or(&e.path()));
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            #[cfg(unix)]
            {
                let link = fs::read_link(e.path()).map_err(|err| Error::io(e.path(), err))?;
                let _ = fs::remove_file(&target);
                std::os::unix::fs::symlink(&link, &target)
                    .map_err(|err| Error::io(&target, err))?;
                count += 1;
            }
            #[cfg(not(unix))]
            {
                // No symlink support: copy the link's target content.
                fs::copy(e.path(), &target).map_err(|err| Error::io(e.path(), err))?;
                count += 1;
            }
        } else if ft.is_dir() {
            count += copy_inner(src_root, &e.path(), dst_root, depth + 1)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
            }
            fs::copy(e.path(), &target).map_err(|err| Error::io(e.path(), err))?;
            count += 1;
        }
    }
    Ok(count)
}

/// Write `data` to `path` via a sibling temp file + rename, so readers
/// never see a half-written file.
pub fn atomic_write(path: &Path, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
    }
    let tmp = path.with_extension(format!(
        "tmp-{}",
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, &e))?;
        f.write_all(data.as_bytes()).map_err(|e| Error::io(&tmp, &e))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).map_err(|e| Error::io(path, &e))?;
    Ok(())
}

/// Remove a directory tree; `Ok(())` when it does not exist.
pub fn remove_tree(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(path, &e)),
    }
}

/// True when `candidate` is inside `base` (after lexical normalization).
/// Used to keep user-supplied paths from escaping the directories they
/// are supposed to stay inside.
pub fn is_subpath(candidate: &Path, base: &Path) -> bool {
    let norm = |p: &Path| -> PathBuf {
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    };
    norm(candidate).starts_with(norm(base))
}

/// Read a file to a string with a path-tagged error.
pub fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io(path, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        for secs in [0u64, 1, 59, 86_399, 86_400, 951_782_400, 1_700_000_000, 4_102_444_799] {
            let iso = iso_from_epoch(secs);
            assert_eq!(parse_iso(&iso), Some(secs), "{iso}");
        }
        assert_eq!(iso_from_epoch(0), "1970-01-01T00:00:00Z");
        // 2026-09-29 00:00:00 UTC
        assert_eq!(parse_iso("2026-09-29T00:00:00Z"), Some(1_790_640_000));
    }

    #[test]
    fn parse_iso_rejects_junk() {
        for s in ["", "2026-09-29", "2026-09-29 00:00:00Z", "2026-13-01T00:00:00Z",
            "2026-09-29T99:00:00Z", "2026-09-29T00:00:00", "xxxx-09-29T00:00:00Z"] {
            assert_eq!(parse_iso(s), None, "{s}");
        }
    }

    #[test]
    fn leap_days() {
        // 2024-02-29 exists; 2023-02-29 does not.
        assert!(parse_iso("2024-02-29T12:00:00Z").is_some());
        assert_eq!(parse_iso("2023-02-29T12:00:00Z"), None);
    }

    #[test]
    fn relative_time() {
        let now = parse_iso("2026-09-29T12:00:00Z").unwrap();
        assert_eq!(rel_time("2026-09-29T11:59:50Z", now), "just now");
        assert_eq!(rel_time("2026-09-29T11:30:00Z", now), "30m ago");
        assert_eq!(rel_time("2026-09-29T06:00:00Z", now), "6h ago");
        assert_eq!(rel_time("2026-09-20T12:00:00Z", now), "9d ago");
        assert_eq!(rel_time("2026-01-01T00:00:00Z", now), "2026-01-01");
        assert_eq!(rel_time("not-a-date", now), "not-a-date");
    }

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/home/u");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("~/x/y", home), PathBuf::from("/home/u/x/y"));
        assert_eq!(expand_tilde("/abs/x", home), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("rel/x", home), PathBuf::from("rel/x"));
        // `~user` is intentionally not expanded — no surprises.
        assert_eq!(expand_tilde("~other/x", home), PathBuf::from("~other/x"));
    }

    #[test]
    fn subpath_detection() {
        assert!(is_subpath(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_subpath(Path::new("/a/b/../b/c"), Path::new("/a/b")));
        assert!(!is_subpath(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!is_subpath(Path::new("/a/b/../.."), Path::new("/a/b")));
    }

    #[test]
    fn walk_and_copy_trees() {
        let dir = std::env::temp_dir().join(format!("beskar-test-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("skill/sub")).unwrap();
        fs::write(dir.join("skill/b.txt"), "b").unwrap();
        fs::write(dir.join("skill/a.txt"), "a").unwrap();
        fs::write(dir.join("skill/sub/c.txt"), "c").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", dir.join("skill/link.txt")).unwrap();

        let items = walk_sorted(&dir).unwrap();
        let rels: Vec<String> = items.iter().map(|i| item_key(i)).collect();
        assert_eq!(
            rels,
            vec!["skill", "skill/a.txt", "skill/b.txt", "skill/link.txt", "skill/sub", "skill/sub/c.txt"]
        );

        let dst = dir.parent().unwrap().join(format!("beskar-test-copy-{}", std::process::id()));
        let n = copy_tree(&dir.join("skill"), &dst).unwrap();
        assert_eq!(n, 4); // 3 files + 1 symlink
        assert_eq!(fs::read_to_string(dst.join("sub/c.txt")).unwrap(), "c");
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(dst.join("link.txt")).unwrap(),
            PathBuf::from("a.txt")
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = std::env::temp_dir().join(format!("beskar-test-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("file.bsk");
        atomic_write(&p, "one\n").unwrap();
        atomic_write(&p, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two\n");
        // No temp litter.
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
