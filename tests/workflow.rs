use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("beskar-test-{}-{n}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, cwd: &Path, args: &[&str], success: bool) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_beskar"))
            .env("BESKAR_HOME", self.path("state with spaces"))
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "args: {args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn end_to_end_drift_and_global_update() {
    let t = Sandbox::new();
    let source = t.path("source").join("review");
    let repo_a = t.path("project a");
    let repo_b = t.path("project b");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir(&repo_a).unwrap();
    fs::create_dir(&repo_b).unwrap();
    fs::write(source.join("SKILL.md"), "one\n").unwrap();
    fs::write(source.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(source.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    t.run(&t.0, &["init"], true);
    t.run(&t.0, &["library", "add", source.to_str().unwrap()], true);
    t.run(&t.0, &["profile", "create", "coding"], true);
    t.run(&t.0, &["profile", "create", "reviewing"], true);
    t.run(&t.0, &["profile", "add", "coding", "review"], true);
    t.run(&t.0, &["profile", "add", "reviewing", "review"], true);
    for repo in [&repo_a, &repo_b] {
        t.run(repo, &["repo", "add"], true);
        t.run(repo, &["repo", "enable", "coding"], true);
        t.run(repo, &["repo", "update"], true);
    }
    t.run(&repo_a, &["repo", "enable", "reviewing"], true);
    let installed_a = repo_a.join(".agents/skills/review/SKILL.md");
    let installed_b = repo_b.join(".agents/skills/review/SKILL.md");
    let canonical = t.path("state with spaces/library/skills/review/SKILL.md");
    assert_eq!(fs::read_to_string(&installed_a).unwrap(), "one\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            fs::metadata(installed_a.with_file_name("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    fs::write(&canonical, "two\n").unwrap();
    fs::write(&installed_a, "local edit\n").unwrap();
    let preview = t.run(&t.0, &["registry", "update", "--all", "--dry-run"], true);
    assert!(preview.contains("local changes would be overwritten"));
    assert_eq!(fs::read_to_string(&installed_b).unwrap(), "one\n");
    t.run(&t.0, &["registry", "update", "--all"], false);
    assert_eq!(fs::read_to_string(&installed_a).unwrap(), "local edit\n");
    assert_eq!(fs::read_to_string(&installed_b).unwrap(), "one\n");
    t.run(
        &t.0,
        &["registry", "update", "--all", "--on-conflict", "keep"],
        true,
    );
    assert_eq!(fs::read_to_string(&installed_a).unwrap(), "local edit\n");
    assert_eq!(fs::read_to_string(&installed_b).unwrap(), "two\n");
    t.run(
        &repo_a,
        &["repo", "update", "--on-conflict", "replace"],
        true,
    );
    assert_eq!(fs::read_to_string(&installed_a).unwrap(), "two\n");
    t.run(&repo_a, &["repo", "disable", "coding"], true);
    t.run(&repo_a, &["repo", "update"], true);
    assert!(installed_a.exists(), "second profile still selects skill");
    t.run(&repo_a, &["repo", "disable", "reviewing"], true);
    fs::write(&installed_a, "second local edit\n").unwrap();
    t.run(&repo_a, &["repo", "update"], false);
    assert_eq!(
        fs::read_to_string(&installed_a).unwrap(),
        "second local edit\n"
    );
    t.run(&repo_a, &["repo", "update", "--on-conflict", "keep"], true);
    assert!(installed_a.exists());
    t.run(
        &repo_a,
        &["repo", "update", "--on-conflict", "replace"],
        true,
    );
    assert!(!installed_a.exists());
    t.run(&t.0, &["doctor"], true);
}

#[test]
fn unmanaged_skill_requires_explicit_policy() {
    let t = Sandbox::new();
    let source = t.path("source").join("manual");
    let repo = t.path("repo");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(repo.join(".agents/skills/manual")).unwrap();
    fs::write(source.join("SKILL.md"), "library").unwrap();
    fs::write(repo.join(".agents/skills/manual/SKILL.md"), "workspace").unwrap();
    t.run(&t.0, &["init"], true);
    t.run(&t.0, &["library", "add", source.to_str().unwrap()], true);
    t.run(&t.0, &["profile", "create", "p"], true);
    t.run(&t.0, &["profile", "add", "p", "manual"], true);
    t.run(&repo, &["repo", "add"], true);
    t.run(&repo, &["repo", "enable", "p"], true);
    t.run(&repo, &["repo", "update"], false);
    assert_eq!(
        fs::read_to_string(repo.join(".agents/skills/manual/SKILL.md")).unwrap(),
        "workspace"
    );
    fs::write(repo.join(".agents/skills/manual/SKILL.md"), "library").unwrap();
    t.run(&repo, &["repo", "update"], false);
    t.run(&repo, &["repo", "update", "--on-conflict", "replace"], true);
    assert_eq!(
        fs::read_to_string(repo.join(".agents/skills/manual/SKILL.md")).unwrap(),
        "library"
    );
}

#[test]
fn non_directory_targets_obey_conflict_policy() {
    let t = Sandbox::new();
    let source = t.path("source").join("manual");
    let repo = t.path("repo");
    let target = repo.join(".agents/skills/manual");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(source.join("SKILL.md"), "library").unwrap();
    fs::write(&target, "workspace file").unwrap();
    t.run(&t.0, &["init"], true);
    t.run(&t.0, &["library", "add", source.to_str().unwrap()], true);
    t.run(&t.0, &["profile", "create", "p"], true);
    t.run(&t.0, &["profile", "add", "p", "manual"], true);
    t.run(&repo, &["repo", "add"], true);
    t.run(&repo, &["repo", "enable", "p"], true);

    let preview = t.run(&repo, &["repo", "update", "--dry-run"], true);
    assert!(preview.contains("non-directory path occupies skill name"));
    t.run(&repo, &["repo", "update"], false);
    t.run(&repo, &["repo", "update", "--on-conflict", "keep"], true);
    assert_eq!(fs::read_to_string(&target).unwrap(), "workspace file");
    t.run(&repo, &["repo", "update", "--on-conflict", "replace"], true);
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "library"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let elsewhere = t.path("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep.txt"), "untouched").unwrap();
        fs::remove_dir_all(&target).unwrap();
        symlink(&elsewhere, &target).unwrap();
        t.run(&repo, &["repo", "update"], false);
        t.run(&repo, &["repo", "update", "--on-conflict", "replace"], true);
        assert!(target.is_dir());
        assert_eq!(
            fs::read_to_string(elsewhere.join("keep.txt")).unwrap(),
            "untouched"
        );
    }

    t.run(&repo, &["repo", "disable", "p"], true);
    fs::remove_dir_all(&target).unwrap();
    fs::write(&target, "local replacement").unwrap();
    t.run(&repo, &["repo", "update"], false);
    assert_eq!(fs::read_to_string(&target).unwrap(), "local replacement");
    t.run(&repo, &["repo", "update", "--on-conflict", "replace"], true);
    assert!(!target.exists());
}
