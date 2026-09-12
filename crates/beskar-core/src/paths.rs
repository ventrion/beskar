//! Filesystem path safety (spec §12, §13, §119).
//!
//! Serialized skill-relative and workspace-relative paths always use `/`
//! (§119); conversion to native separators happens only at filesystem
//! boundaries. Path traversal through `..` MUST be rejected (§12), and
//! Windows-reserved names MUST NOT be accepted for bucket segments (§13).

use crate::Error;

/// Windows reserved device names (case-insensitive, with or without an
/// extension). Creating such files fails on Windows, so Beskar rejects them
/// everywhere it names paths (§13).
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether a path segment is a Windows reserved device name (§13).
/// The check applies to the base name before any extension: `aux.txt` is as
/// uncreatable on Windows as `aux`.
pub fn is_windows_reserved(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    WINDOWS_RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(base))
}

/// Validates a `/`-separated relative path for portable use (§12, §13, §119).
///
/// Rules: non-empty; no absolute forms (`/`, `\`, `C:`); no backslashes or
/// NUL bytes; segments are non-empty, not `.`/`..`, not Windows-reserved,
/// and contain no control characters; no trailing dot or space on a segment
/// (Windows forbids creating such files).
pub fn validate_relative_path(path: &str) -> crate::Result<()> {
    if path.is_empty() {
        return Err(Error::path_safety("path must not be empty"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(Error::path_safety(format!(
            "path {path:?} must be relative"
        )));
    }
    if path.contains('\\') {
        return Err(Error::path_safety(format!(
            "path {path:?} must use '/' separators, not '\\'"
        )));
    }
    if path.contains('\0') {
        return Err(Error::path_safety("path must not contain NUL bytes"));
    }
    if path.as_bytes().get(1) == Some(&b':') {
        return Err(Error::path_safety(format!(
            "path {path:?} must not contain a drive prefix"
        )));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(Error::path_safety(format!(
                "path {path:?} has an empty segment (double or trailing '/')"
            )));
        }
        if segment == "." || segment == ".." {
            return Err(Error::path_safety(format!(
                "path {path:?} must not contain {segment:?} traversal segments"
            )));
        }
        if is_windows_reserved(segment) {
            return Err(Error::path_safety(format!(
                "path segment {segment:?} is a Windows reserved device name"
            )));
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(Error::path_safety(format!(
                "path segment {segment:?} must not end with '.' or ' ' (Windows)"
            )));
        }
        if segment.chars().any(char::is_control) {
            return Err(Error::path_safety(format!(
                "path segment {segment:?} must not contain control characters"
            )));
        }
    }
    Ok(())
}

/// Validates a bucket path: the Library-relative directory hierarchy beneath
/// `skills/` (spec §13). An empty bucket (a skill directly under `skills/`)
/// is valid.
///
/// Accepted v1 segments consist of portable lowercase ASCII `a-z 0-9 . _ -`
/// and, in addition to the [`validate_relative_path`] rules, MUST NOT equal
/// `.` or `..`, contain path separators of their own, use absolute paths, or
/// use Windows reserved device names.
pub fn validate_bucket(bucket: &str) -> crate::Result<()> {
    if bucket.is_empty() {
        return Ok(());
    }
    validate_relative_path(bucket)?;
    for segment in bucket.split('/') {
        if !segment
            .chars()
            .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-'))
        {
            return Err(Error::path_safety(format!(
                "bucket segment {segment:?} must use only portable lowercase \
                 a-z, 0-9, '.', '_' and '-'"
            )));
        }
    }
    Ok(())
}

/// Splits a `/`-separated path into (parent path, leaf segment).
pub fn split_leaf(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((parent, leaf)) => (parent, leaf),
        None => ("", path),
    }
}

/// Converts a `/`-separated relative path into a native [`PathBuf`] at the
/// filesystem boundary (spec §119). Serialized paths always use `/`;
/// conversion to native separators happens only here.
pub fn to_native_path(relative: &str) -> std::path::PathBuf {
    relative.split('/').collect()
}

/// Converts a native relative path into its `/`-separated string form at the
/// filesystem boundary (spec §119) — the inverse of [`to_native_path`].
/// Native `PathBuf` stringification must never be assumed to round-trip
/// through the serialized form: on Windows `to_str()` yields backslash
/// separators, which validation and stamps reject. This is the only
/// sanctioned native → serialized conversion; non-UTF-8 paths fail closed
/// (§4) instead of being lossily mangled into serialized state.
pub fn to_slash_path(native: &std::path::Path) -> crate::Result<String> {
    Ok(native
        .iter()
        .map(|component| {
            component
                .to_str()
                .ok_or_else(|| crate::Error::path_safety("non-UTF-8 path at a filesystem boundary"))
        })
        .collect::<crate::Result<Vec<_>>>()?
        .join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_bucket(bucket: &str) {
        validate_bucket(bucket)
            .unwrap_or_else(|e| panic!("bucket {bucket:?} should be valid: {e}"));
    }

    fn err_bucket(bucket: &str) {
        assert!(
            validate_bucket(bucket).is_err(),
            "bucket {bucket:?} should be invalid"
        );
    }

    #[test]
    fn accepts_portable_buckets() {
        ok_bucket("");
        ok_bucket("rust");
        ok_bucket("engineering/process");
        ok_bucket("a.b_c-d");
        ok_bucket("deep/nested/bucket/path");
    }

    #[test]
    fn rejects_traversal_segments() {
        err_bucket("..");
        err_bucket(".");
        err_bucket("engineering/../process");
        err_bucket("a/../b");
    }

    #[test]
    fn rejects_absolute_and_separator_forms() {
        err_bucket("/absolute");
        err_bucket("with\\backslash");
        err_bucket("double//slash");
        err_bucket("trailing/");
        err_bucket("leading/");
    }

    #[test]
    fn rejects_non_portable_segments() {
        err_bucket("Engineering");
        err_bucket("café");
        err_bucket("sp ace");
        err_bucket("q!");
    }

    #[test]
    fn rejects_windows_reserved_names() {
        assert!(is_windows_reserved("con"));
        assert!(is_windows_reserved("AUX"));
        assert!(is_windows_reserved("com1"));
        assert!(is_windows_reserved("aux.txt"));
        assert!(!is_windows_reserved("auxx"));
        assert!(!is_windows_reserved("com10"));
        assert!(!is_windows_reserved(".agents"));
        err_bucket("aux");
        err_bucket("engineering/con");
        err_bucket("nul.txt");
    }

    #[test]
    fn rejects_windows_hostile_segment_shapes() {
        err_bucket("trailing.");
        err_bucket("trailing ");
    }

    #[test]
    fn relative_paths_accept_targets_and_reject_unsafe_forms() {
        validate_relative_path(".agents/skills").expect("target is valid");
        validate_relative_path("x.txt").expect("simple file");
        for bad in [
            "", "/abs", "C:/x", "a\\b", "a/../b", "..", "a//b", "con", "a/b/", "end.",
        ] {
            assert!(
                validate_relative_path(bad).is_err(),
                "path {bad:?} should be invalid"
            );
        }
    }

    #[test]
    fn windows_style_backslash_inputs_are_rejected_as_serialized_paths() {
        // §119: serialized paths always use '/'. Windows-native forms must
        // never be accepted as serialized state, so a native path that
        // slipped through unconverted fails loudly here instead of silently
        // corrupting registry entries, stamps, or installed layouts.
        for bad in [
            "skills\\testing",          // plain backslash separator
            "skills\\nested\\SKILL.md", // deep backslash path
            "a\\..\\b",                 // traversal via backslashes
            "C:\\Users\\x",             // drive-absolute
            "C:skills",                 // drive-relative
            "\\\\server\\share",        // UNC prefix
            "\\absolute",               // backslash-rooted
            "trailing\\",               // trailing separator
            "skills\\CON",              // reserved name, backslash form
            "skills\\aux.txt",          // reserved base, backslash form
            "skills\\trailing.",        // trailing dot, backslash form
            "skills\\trailing ",        // trailing space, backslash form
        ] {
            assert!(
                validate_relative_path(bad).is_err(),
                "path {bad:?} should be invalid"
            );
        }
        // The forward-slash equivalents stay acceptable (pure validation on
        // the string level — provable on every platform).
        validate_relative_path("skills/testing/SKILL.md").expect("portable form");
        validate_relative_path("skills/trailing.dot-free").expect("dot inside is fine");
    }

    #[test]
    fn windows_reserved_and_hostile_shapes_rejected_for_all_paths() {
        // §13 rules apply to every Beskar-named path, not just buckets.
        for bad in [
            "aux", "AUX", "aux.txt", "com1", "end.", "end ", "ok/CON", "a/b/nul",
        ] {
            assert!(
                validate_relative_path(bad).is_err(),
                "path {bad:?} should be invalid"
            );
        }
    }

    #[test]
    fn to_slash_path_normalizes_native_separators() {
        // §119 inverse of to_native_path: built component-wise so the input
        // uses the platform separator idiomatically; the output must always
        // be the '/'-separated serialized form.
        let native = std::path::PathBuf::from("a").join("b").join("SKILL.md");
        assert_eq!(to_slash_path(&native).expect("utf-8"), "a/b/SKILL.md");
        assert_eq!(
            to_slash_path(std::path::Path::new("leaf")).expect("utf-8"),
            "leaf"
        );
        assert_eq!(to_slash_path(std::path::Path::new("")).expect("utf-8"), "");
    }

    #[cfg(unix)]
    #[test]
    fn to_slash_path_fails_closed_on_non_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let native = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"a/\xff\xfe"));
        assert!(to_slash_path(&native).is_err());
    }

    #[test]
    fn split_leaf_separates_parent_and_leaf() {
        assert_eq!(split_leaf("skills/a/b"), ("skills/a", "b"));
        assert_eq!(split_leaf("leaf"), ("", "leaf"));
    }

    #[test]
    fn to_native_path_converts_at_fs_boundary() {
        assert_eq!(to_native_path("a/b/c"), std::path::PathBuf::from("a/b/c"));
        let native = to_native_path("a/b");
        #[cfg(unix)]
        assert_eq!(native, std::path::PathBuf::from("a/b"));
        assert_eq!(native.iter().count(), 2);
    }
}
