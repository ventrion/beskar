//! Filesystem fixtures for hermetic tests (spec §125).

use std::path::{Path, PathBuf};

/// Creates a file with `content` (and any missing parents) under `root`.
pub fn write_file(root: &Path, relative: &str, content: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parents");
    }
    std::fs::write(&path, content).expect("write file");
    path
}

/// Creates a skill directory with a minimal valid `SKILL.md`.
pub fn write_skill(root: &Path, bucket_and_name: &str, description: &str) -> PathBuf {
    let name = bucket_and_name
        .rsplit('/')
        .next()
        .unwrap_or(bucket_and_name);
    write_file(
        root,
        &format!("{bucket_and_name}/SKILL.md"),
        &format!("---\nname: {name}\ndescription: {description}\n---\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TempRoot;
    use beskar_core::skill::SkillFrontmatter;
    use std::assert_eq;

    #[test]
    fn written_skill_parses_as_valid_frontmatter() {
        let root = TempRoot::new();
        write_skill(
            root.path(),
            "engineering/process/code-review",
            "Reviews code",
        );
        let raw =
            std::fs::read_to_string(root.path().join("engineering/process/code-review/SKILL.md"))
                .expect("read");
        let fm = SkillFrontmatter::parse(&raw).expect("valid");
        assert_eq!(fm.name.as_str(), "code-review");
    }
}
