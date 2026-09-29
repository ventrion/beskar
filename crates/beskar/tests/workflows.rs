use beskar::{app::App, fs as bfs, reconcile};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
}
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beskar-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("state # with spaces");
        let repo = root.join("workspace with spaces");
        fs::create_dir_all(&repo).unwrap();
        let s = Self { root, home, repo };
        s.ok(&["init"]);
        s
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_beskar"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn err(&self, args: &[&str], expected: &str) {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "command unexpectedly succeeded: {args:?}"
        );
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains(expected),
            "expected {expected:?}: {combined}"
        );
    }
    fn library(&self, name: &str) -> PathBuf {
        self.home.join("library/skills").join(name)
    }
    fn installed(&self, name: &str) -> PathBuf {
        self.repo.join(".agents/skills").join(name)
    }
    fn add(&self, name: &str) -> PathBuf {
        let source = self.root.join("sources").join(name);
        fs::create_dir_all(source.join("references/empty")).unwrap();
        fs::write(
            source.join("SKILL.md"),
            format!("# {name}\nOriginal content\n"),
        )
        .unwrap();
        fs::write(source.join("references/data.bin"), [0, 1, 0xff, 2]).unwrap();
        self.ok(&["library", "add", source.to_str().unwrap()]);
        source
    }
    fn setup(&self) {
        self.add("review");
        self.ok(&["profile", "create", "coding"]);
        self.ok(&["profile", "add", "coding", "review"]);
        self.ok(&["repo", "add"]);
        self.ok(&["repo", "enable", "coding"]);
        self.ok(&["repo", "update"]);
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn files(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.push((
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                ));
            }
        }
    }
    let mut result = Vec::new();
    visit(path, path, &mut result);
    result.sort();
    result
}

#[test]
fn complete_workflow_unions_profiles_and_updates_copies() {
    let s = Sandbox::new();
    s.add("review");
    s.add("testing");
    s.ok(&["profile", "create", "coding"]);
    s.ok(&["profile", "create", "research"]);
    s.ok(&["profile", "add", "coding", "review", "testing"]);
    s.ok(&["profile", "add", "research", "review"]);
    s.ok(&["repo", "add"]);
    s.ok(&["repo", "enable", "coding", "research"]);
    assert!(
        !s.installed("review").exists(),
        "enable should change only desired state"
    );
    let before = files(&s.root);
    s.ok(&["repo", "update", "--dry-run"]);
    assert_eq!(files(&s.root), before);
    s.ok(&["repo", "update"]);
    assert_eq!(
        fs::read_dir(s.repo.join(".agents/skills")).unwrap().count(),
        2
    );
    assert!(
        !fs::symlink_metadata(s.installed("review"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        bfs::fingerprint(&s.installed("review")).unwrap(),
        bfs::fingerprint(&s.library("review")).unwrap()
    );
    let state = fs::read(s.home.join("registry.bsk")).unwrap();
    let modified = fs::metadata(s.installed("review").join("SKILL.md"))
        .unwrap()
        .modified()
        .unwrap();
    s.ok(&["repo", "update"]);
    assert_eq!(state, fs::read(s.home.join("registry.bsk")).unwrap());
    assert_eq!(
        modified,
        fs::metadata(s.installed("review").join("SKILL.md"))
            .unwrap()
            .modified()
            .unwrap()
    );
    s.ok(&["repo", "disable", "coding"]);
    s.ok(&["repo", "update"]);
    assert!(s.installed("review").exists());
    assert!(!s.installed("testing").exists());
    s.ok(&["doctor"]);
    assert!(
        s.ok(&["registry", "list", "--skill", "review", "--json"])
            .contains("research")
    );
    assert!(
        s.ok(&["registry", "stats", "--json"])
            .contains("\"installed_skills\":1")
    );
}

#[test]
fn local_edits_require_policy_and_keep_does_not_reset_baseline() {
    let s = Sandbox::new();
    s.setup();
    let local = s.installed("review").join("SKILL.md");
    fs::write(&local, "my edits").unwrap();
    fs::write(s.library("review").join("SKILL.md"), "library edits").unwrap();
    let state = fs::read(s.home.join("registry.bsk")).unwrap();
    s.err(&["repo", "update"], "local changes");
    assert_eq!(fs::read_to_string(&local).unwrap(), "my edits");
    assert_eq!(state, fs::read(s.home.join("registry.bsk")).unwrap());
    let dry = s.run(&["repo", "update", "--dry-run", "--json"]);
    assert_eq!(dry.status.code(), Some(3));
    assert!(
        String::from_utf8(dry.stdout)
            .unwrap()
            .contains("local-changes")
    );
    s.ok(&["repo", "update", "--conflict", "keep"]);
    assert_eq!(state, fs::read(s.home.join("registry.bsk")).unwrap());
    assert_eq!(s.run(&["status"]).status.code(), Some(3));
    s.ok(&["repo", "update", "--conflict", "replace"]);
    assert_eq!(fs::read_to_string(&local).unwrap(), "library edits");
    s.ok(&["status"]);
    fs::write(&local, "more local edits").unwrap();
    s.ok(&["repo", "disable", "coding"]);
    s.err(&["repo", "update"], "local changes");
    s.ok(&["repo", "update", "--conflict", "keep"]);
    assert!(local.exists());
    s.ok(&["repo", "update", "--conflict", "replace"]);
    assert!(!local.exists());
}

#[test]
fn global_preflight_prevents_partial_changes_and_updates_only_selected_skills() {
    let s = Sandbox::new();
    s.setup();
    let second = s.root.join("z-other");
    fs::create_dir(&second).unwrap();
    s.ok(&["repo", "add", second.to_str().unwrap()]);
    s.ok(&[
        "repo",
        "enable",
        "coding",
        "--repo",
        second.to_str().unwrap(),
    ]);
    s.ok(&["registry", "update", "--all"]);
    let first = s.installed("review").join("SKILL.md");
    let original = fs::read(&first).unwrap();
    let other = second.join(".agents/skills/review/SKILL.md");
    fs::write(&other, "local in other repo").unwrap();
    fs::write(s.library("review").join("SKILL.md"), "canonical v2").unwrap();
    s.err(&["update", "--all"], "no workspaces changed");
    assert_eq!(original, fs::read(&first).unwrap());
    s.ok(&["update", "--all", "--conflict", "keep"]);
    assert_eq!(fs::read_to_string(&first).unwrap(), "canonical v2");
    assert_eq!(fs::read_to_string(&other).unwrap(), "local in other repo");
    s.ok(&["update", "--all", "--conflict", "replace"]);
    s.ok(&["registry", "status"]);
}

#[test]
fn unmanaged_paths_are_never_adopted_and_unrelated_skills_are_left_alone() {
    let s = Sandbox::new();
    s.add("review");
    s.ok(&["profile", "create", "coding"]);
    s.ok(&["profile", "add", "coding", "review"]);
    s.ok(&["repo", "add"]);
    s.ok(&["repo", "enable", "coding"]);
    fs::create_dir_all(s.installed("review")).unwrap();
    fs::write(s.installed("review").join("mine"), "mine").unwrap();
    s.err(&["update", "--conflict", "replace"], "unmanaged");
    s.err(&["update", "--conflict", "keep"], "unmanaged");
    assert_eq!(
        fs::read_to_string(s.installed("review").join("mine")).unwrap(),
        "mine"
    );
    fs::rename(s.installed("review"), s.installed("personal")).unwrap();
    s.ok(&["update"]);
    s.ok(&["repo", "disable", "coding"]);
    s.ok(&["update"]);
    assert!(s.installed("personal").join("mine").exists());
}

#[test]
fn scan_is_explicit_and_batch_import_is_validated_before_writes() {
    let s = Sandbox::new();
    let source = s.root.join("scan");
    for name in ["one", "two"] {
        fs::create_dir_all(source.join(name)).unwrap();
        fs::write(source.join(name).join("SKILL.md"), "skill").unwrap();
    }
    let preview = s.ok(&[
        "library",
        "scan",
        source.to_str().unwrap(),
        "--dry-run",
        "--json",
    ]);
    assert!(preview.contains("two"));
    assert!(!s.library("one").exists());
    s.err(&["library", "scan", source.to_str().unwrap()], "--yes");
    s.ok(&["library", "scan", source.to_str().unwrap(), "--yes"]);
    assert!(s.library("two").exists());
    s.err(
        &["library", "scan", source.to_str().unwrap(), "--yes"],
        "already exists",
    );
    let duplicate = s.root.join("duplicate");
    for name in ["a/new", "b/new"] {
        fs::create_dir_all(duplicate.join(name)).unwrap();
        fs::write(duplicate.join(name).join("SKILL.md"), "skill").unwrap();
    }
    s.err(
        &["library", "scan", duplicate.to_str().unwrap(), "--yes"],
        "duplicate imported name",
    );
    assert!(!s.library("new").exists());
}

#[test]
fn reference_guards_profile_comments_and_unregister_semantics() {
    let s = Sandbox::new();
    s.setup();
    s.err(&["library", "remove", "review"], "referenced by profile");
    s.err(&["profile", "delete", "coding"], "enabled in");
    let profile = s.home.join("library/profiles/coding.bsk");
    fs::write(
        &profile,
        "# Keep this explanation\nbeskar 1\n\n# Reviews\nskill review\n",
    )
    .unwrap();
    s.add("testing");
    s.ok(&["profile", "add", "coding", "testing"]);
    s.ok(&["profile", "remove", "coding", "testing"]);
    assert!(
        fs::read_to_string(&profile)
            .unwrap()
            .starts_with("# Keep this explanation\n")
    );
    s.ok(&["repo", "remove"]);
    assert!(s.installed("review").exists());
    s.ok(&["profile", "delete", "coding"]);
    s.ok(&["library", "remove", "review"]);
    assert!(s.installed("review").exists());
}

#[test]
fn missing_workspaces_are_reported_and_pruned_only_when_requested() {
    let s = Sandbox::new();
    s.setup();
    let missing = s.root.join("removed");
    fs::create_dir(&missing).unwrap();
    s.ok(&["repo", "add", missing.to_str().unwrap()]);
    fs::remove_dir(&missing).unwrap();
    assert_eq!(s.run(&["registry", "status"]).status.code(), Some(3));
    s.err(&["doctor"], "directory is missing");
    let before = fs::read(s.home.join("registry.bsk")).unwrap();
    s.ok(&["registry", "prune", "--dry-run"]);
    assert_eq!(before, fs::read(s.home.join("registry.bsk")).unwrap());
    s.ok(&["registry", "prune"]);
    s.ok(&["doctor"]);
    assert!(s.installed("review").exists());
}

#[test]
fn malformed_config_profiles_and_path_traversal_fail_closed() {
    let s = Sandbox::new();
    s.setup();
    s.err(&["profile", "create", "../escape"], "invalid name");
    let path = s.home.join("library/profiles/coding.bsk");
    fs::write(&path, "beskar 1\nskill review\nskill review\n").unwrap();
    s.err(&["update"], "line 3");
    assert!(s.installed("review").exists());
    let config = s.home.join("config.bsk");
    let original = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        original.replace("agent-skills .agents/skills", "agent-skills ../escape"),
    )
    .unwrap();
    s.err(&["doctor"], "relative path");
}

#[test]
fn lock_excludes_concurrent_processes_without_stale_lock_cleanup() {
    let s = Sandbox::new();
    let app = App::open(&s.home).unwrap();
    s.err(&["profile", "create", "busy"], "locked");
    drop(app);
    s.ok(&["profile", "create", "busy"]);
}

#[test]
fn api_refuses_stale_plans() {
    let s = Sandbox::new();
    s.setup();
    let mut app = App::open(&s.home).unwrap();
    let plan = reconcile::plan(&app, &s.repo).unwrap();
    fs::write(s.installed("review").join("new"), "edited after plan").unwrap();
    assert!(
        reconcile::apply(&mut app, &[plan], reconcile::ConflictPolicy::Replace)
            .unwrap_err()
            .to_string()
            .contains("changed after planning")
    );
    assert!(s.installed("review").join("new").exists());
}

#[test]
fn registry_write_failure_rolls_back_all_workspace_files() {
    let s = Sandbox::new();
    s.setup();
    let before = files(&s.repo);
    fs::write(s.library("review").join("SKILL.md"), "new library version").unwrap();
    let mut app = App::open(&s.home).unwrap();
    let blocked = s.home.join("blocked-registry");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("file"), "prevent replacement").unwrap();
    app.config.registry = blocked;
    let plan = reconcile::plan(&app, &s.repo).unwrap();
    let error = reconcile::apply(&mut app, &[plan], reconcile::ConflictPolicy::Fail).unwrap_err();
    assert!(error.to_string().contains("rolled back"), "{error}");
    // Exclude preserved sibling transaction directories from the managed tree comparison.
    assert_eq!(
        files(&s.installed("review")),
        before
            .into_iter()
            .filter_map(|(p, b)| p
                .strip_prefix(".agents/skills/review")
                .ok()
                .map(|p| (p.to_owned(), b)))
            .collect::<Vec<_>>()
    );
    assert!(!app.pending_path().exists());
}

#[test]
fn interruption_marker_blocks_mutations_and_is_visible_to_doctor() {
    let s = Sandbox::new();
    s.setup();
    fs::write(
        s.home.join("registry.pending.bsk"),
        "beskar 1\njournal /some/preserved/location\n",
    )
    .unwrap();
    s.err(&["update"], "interrupted update");
    s.err(&["repo", "disable", "coding"], "interrupted update");
    s.err(&["doctor"], "interrupted update");
    assert!(s.installed("review").exists());
}

#[test]
fn help_version_and_cli_errors_are_available_without_initialization() {
    let s = Sandbox::new();
    assert!(s.ok(&["--version"]).starts_with("beskar 0.1.0"));
    assert!(s.ok(&["repo", "update", "--help"]).contains("--conflict"));
    assert_eq!(s.run(&["--wat", "--json"]).status.code(), Some(2));
    assert!(
        String::from_utf8(s.run(&["--wat", "--json"]).stdout)
            .unwrap()
            .contains("\"ok\":false")
    );
    assert_eq!(s.run(&["registry", "update"]).status.code(), Some(2));
    s.err(&["library", "list", "--all"], "not valid");
}

#[cfg(unix)]
#[test]
fn symlinks_in_sources_and_workspace_boundaries_are_rejected() {
    use std::os::unix::fs::symlink;
    let s = Sandbox::new();
    s.setup();
    let source = s.root.join("unsafe");
    fs::create_dir(&source).unwrap();
    symlink(s.library("review").join("SKILL.md"), source.join("link")).unwrap();
    s.err(&["library", "add", source.to_str().unwrap()], "symlink");
    assert!(!s.library("unsafe").exists());
    let alias = s.root.join("alias");
    symlink(&source, &alias).unwrap();
    s.err(&["library", "add", alias.to_str().unwrap()], "symlink");
    let saved = s.root.join("saved-agents");
    fs::rename(s.repo.join(".agents"), &saved).unwrap();
    symlink(&saved, s.repo.join(".agents")).unwrap();
    s.err(&["update", "--conflict", "replace"], "symlink");
    assert!(saved.join("skills/review/SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn executable_bits_and_empty_directories_are_copied_and_fingerprinted() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new();
    s.setup();
    let script = s.library("review").join("script.sh");
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    s.ok(&["update"]);
    let installed = s.installed("review").join("script.sh");
    assert_eq!(
        fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
        0o755
    );
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o644)).unwrap();
    s.err(&["update"], "local changes");
    s.ok(&["update", "--conflict", "replace"]);
    fs::remove_dir(s.installed("review").join("references/empty")).unwrap();
    s.err(&["update"], "local changes");
}

#[test]
fn library_cannot_become_an_automatically_discovered_skill_collection() {
    let s = Sandbox::new();
    for (i, library) in [
        s.root.join(".agents"),
        s.root.join(".agents/skills/library"),
        s.root.join(".claude"),
        s.root.join(".cursor/skills"),
    ]
    .iter()
    .enumerate()
    {
        let home = s.root.join(format!("invalid-home-{i}"));
        let result = App::initialize(&home, Some(library));
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("agent's skills directory")
        );
        assert!(!library.join("skills").exists());
    }
}

#[test]
fn changing_destination_requires_removing_tracked_installations_first() {
    let s = Sandbox::new();
    s.setup();
    let config = s.home.join("config.bsk");
    let original = fs::read_to_string(&config).unwrap();
    let changed = original.replace("agent-skills .agents/skills", "agent-skills .custom/skills");
    fs::write(&config, &changed).unwrap();
    s.err(&["update"], "agent-skills changed");
    assert!(s.installed("review").exists());
    assert!(!s.repo.join(".custom").exists());
    fs::write(&config, &original).unwrap();
    s.ok(&["repo", "disable", "coding"]);
    s.ok(&["update"]);
    fs::write(&config, &changed).unwrap();
    s.ok(&["repo", "enable", "coding"]);
    s.ok(&["update"]);
    assert!(s.repo.join(".custom/skills/review/SKILL.md").exists());
    s.ok(&["doctor"]);
}

#[test]
fn imports_accept_relative_parent_paths_and_do_not_require_skill_metadata() {
    let s = Sandbox::new();
    fs::create_dir(s.root.join("plain")).unwrap();
    fs::write(s.root.join("plain/file"), "opaque content").unwrap();
    s.ok(&["library", "add", "../plain"]);
    assert!(s.library("plain").join("file").exists());
    assert!(!s.library("plain").join("SKILL.md").exists());
}

#[test]
fn converged_local_and_library_edits_record_baseline_without_rewriting_files() {
    let s = Sandbox::new();
    s.setup();
    let target = s.installed("review").join("SKILL.md");
    fs::write(&target, "same edit on both sides").unwrap();
    fs::write(
        s.library("review").join("SKILL.md"),
        "same edit on both sides",
    )
    .unwrap();
    let modified = fs::metadata(&target).unwrap().modified().unwrap();
    let output = s.ok(&["update", "--json"]);
    assert!(output.contains("\"action\":\"record\""));
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
    s.ok(&["doctor"]);
}

#[test]
fn replace_repairs_local_only_drift_even_with_the_same_recorded_fingerprint() {
    let s = Sandbox::new();
    s.setup();
    let target = s.installed("review").join("SKILL.md");
    let canonical = fs::read(s.library("review").join("SKILL.md")).unwrap();
    for _ in 0..3 {
        fs::write(&target, "local-only edit").unwrap();
        s.ok(&["update", "--conflict", "replace"]);
        assert_eq!(fs::read(&target).unwrap(), canonical);
        s.ok(&["status"]);
    }
}

#[test]
fn missing_managed_copy_is_reinstalled_even_with_an_unchanged_baseline() {
    let s = Sandbox::new();
    s.setup();
    fs::remove_dir_all(s.installed("review")).unwrap();
    s.ok(&["update"]);
    assert_eq!(
        bfs::fingerprint(&s.installed("review")).unwrap(),
        bfs::fingerprint(&s.library("review")).unwrap()
    );
    s.ok(&["doctor"]);
}
