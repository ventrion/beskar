//! End-to-end tests of what goes wrong around Beskar: other processes,
//! interrupted runs, links, odd arguments, closed output, broken files and
//! people answering prompts through a pipe.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::{DEAD_PID, Doc, LIB, Run, World};

// ----- Helpers -----

/// Run `beskar` with extra environment variables.
fn run_env(w: &World, cwd: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
    let mut command = w.command(cwd, args);
    for (key, value) in env {
        command.env(key, value);
    }
    Run::of(command.output().unwrap())
}

/// Take Beskar's lock from this process, as another beskar would, and say
/// who holds it the way beskar does.
fn hold_lock(w: &World) -> fs::File {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(w.path(".beskar/lock"))
        .unwrap();
    file.lock().unwrap();
    file.set_len(0).unwrap();
    file.write_all(b"pid: 1\nsince: 2026-01-01T00:00:00Z\ncommand: test holder\n")
        .unwrap();
    file
}

/// A skill changed both in workspace `api` and in the library.
fn diverged() -> World {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.append("api/.agents/skills/git/SKILL.md", "local tweak\n");
    w.append(&format!("{LIB}/git/SKILL.md"), "library change\n");
    w
}

/// Everything read from `stream`, chunk by chunk, on a channel.
fn watch(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0
                || tx
                    .send(String::from_utf8_lossy(&buf[..n]).into_owned())
                    .is_err()
            {
                break;
            }
        }
    });
    rx
}

/// Wait until `text` has appeared `times` times in what `rx` delivers.
fn wait_for(rx: &mpsc::Receiver<String>, seen: &mut String, text: &str, times: usize) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen.matches(text).count() < times {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(chunk) => seen.push_str(&chunk),
            Err(_) => panic!("never saw {text:?} {times} times:\n{seen}"),
        }
    }
}

/// Temporary entries Beskar leaves when interrupted, anywhere under `rel`.
fn leftovers(w: &World, rel: &str) -> Vec<String> {
    w.tree(rel)
        .into_keys()
        .filter(|path| path.split('/').any(|part| part.starts_with(".beskar")))
        .collect()
}

// ----- Several processes at once -----

#[test]
fn concurrent_updates_of_a_large_library_both_succeed() {
    let w = World::with_library();
    w.write(
        "big-src/big/SKILL.md",
        "---\nname: big\ndescription: Many files\n---\n",
    );
    for i in 0..300 {
        w.write(
            &format!("big-src/big/refs/d{:02}/f{i:03}.md", i % 12),
            &format!("# Reference {i}\n{}\n", "Some text. ".repeat(20)),
        );
    }
    w.run(".", &["library", "add", "big-src/big"]).ok();
    w.run(".", &["profile", "add", "coding", "big"]).ok();
    let workspaces = ["ws/a", "ws/b", "ws/c"];
    for ws in workspaces {
        w.run(ws, &["repo", "add", "."]).ok();
        w.run(ws, &["repo", "enable", "coding"]).ok();
    }
    for round in 0..3 {
        match round {
            1 => {
                w.append(&format!("{LIB}/big/refs/d03/f003.md"), "Changed.\n");
                w.write(&format!("{LIB}/big/refs/new.md"), "New file.\n");
                fs::remove_file(w.path(&format!("{LIB}/big/refs/d07/f007.md"))).unwrap();
            }
            2 => w.append(&format!("{LIB}/big/SKILL.md"), "More.\n"),
            _ => {}
        }
        let children: Vec<_> = (0..2)
            .map(|_| {
                w.command(".", &["--json", "update", "--all"])
                    .spawn()
                    .unwrap()
            })
            .collect();
        let docs: Vec<Doc> = children
            .into_iter()
            .map(|child| w.doc(Run::of(child.wait_with_output().unwrap())).ok())
            .collect();
        for ws in workspaces {
            // Each workspace is planned under the lock, so exactly one of
            // the two processes changed it.
            let mut results: Vec<&str> = docs
                .iter()
                .map(|doc| doc["data"]["repos"].find("repo", &w.abs(ws))["result"].as_str())
                .collect();
            results.sort_unstable();
            assert_eq!(results, ["applied", "up_to_date"], "round {round}, {ws}");
        }
        let fingerprint = w.json(".", &["library", "show", "big"]).ok()["data"]["fingerprint"]
            .as_str()
            .to_string();
        let registry = w.json(".", &["registry", "list"]).ok();
        let library = w.tree(&format!("{LIB}/big"));
        assert!(library.len() > 300);
        for ws in workspaces {
            let entry = registry["data"].find("path", &w.abs(ws));
            let big = entry["installed"].find("skill", "big");
            assert_eq!(big["base"].as_str(), fingerprint, "round {round}, {ws}");
            assert_eq!(
                w.list(&format!("{ws}/.agents/skills")),
                ["big", "code-review", "git", "testing"]
            );
            assert!(
                w.tree(&format!("{ws}/.agents/skills/big")) == library,
                "{ws}"
            );
        }
        assert!(
            leftovers(&w, ".beskar").is_empty(),
            "{:?}",
            leftovers(&w, ".beskar")
        );
        assert!(leftovers(&w, "ws").is_empty(), "{:?}", leftovers(&w, "ws"));
    }
    let doc = w.json(".", &["update", "--all"]).ok();
    assert_eq!(doc["data"]["summary"]["up_to_date"].as_i64(), 3);
}

#[test]
fn concurrent_edits_of_one_profile_are_all_kept() {
    let w = World::with_library();
    for i in 0..8 {
        w.skill("more", &format!("s{i}"), "One of many");
    }
    w.run(".", &["library", "scan", "more", "--yes"]).ok();
    w.run(".", &["profile", "create", "many"]).ok();
    let children: Vec<_> = (0..8)
        .map(|i| {
            w.command(".", &["profile", "add", "many", &format!("s{i}")])
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        Run::of(child.wait_with_output().unwrap()).ok();
    }
    let doc = w.json(".", &["profile", "show", "many"]).ok();
    let mut skills = doc["data"]["skills"].strs();
    skills.sort_unstable();
    assert_eq!(skills, ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"]);
}

#[test]
fn concurrent_registrations_are_all_kept() {
    let w = World::with_library();
    let names: Vec<String> = (0..8).map(|i| format!("ws/w{i}")).collect();
    let children: Vec<_> = names
        .iter()
        .map(|name| w.command(name, &["repo", "add", "."]).spawn().unwrap())
        .collect();
    for child in children {
        Run::of(child.wait_with_output().unwrap()).ok();
    }
    let doc = w.json(".", &["repo", "list"]).ok();
    let mut paths: Vec<&str> = doc["data"]
        .as_array()
        .iter()
        .map(|e| e["path"].as_str())
        .collect();
    paths.sort_unstable();
    let expected: Vec<String> = names.iter().map(|n| w.abs(n)).collect();
    assert_eq!(paths, expected);
}

#[test]
fn with_no_patience_a_locked_home_fails_at_once() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let profile = ".beskar/library/profiles/coding.bsk";
    let profile_before = w.read(profile);
    let registry_before = w.read(".beskar/registry.bsk");
    let no_wait = [("BESKAR_LOCK_TIMEOUT", "0")];
    let lock = hold_lock(&w);
    let start = Instant::now();

    let doc = w
        .doc(run_env(&w, "api", &["--json", "update"], &no_wait))
        .code(1);
    let error = &doc["data"]["repos"][0]["error"];
    assert_eq!(error["kind"].as_str(), "locked", "{}", doc.stdout);
    assert!(
        error["message"].as_str().contains("command test holder"),
        "{error}"
    );
    let doc = w
        .doc(run_env(
            &w,
            ".",
            &["--json", "profile", "add", "coding", "pdf"],
            &no_wait,
        ))
        .code(1);
    assert_eq!(doc["error"]["kind"].as_str(), "locked");
    assert!(
        doc["error"]["hints"]
            .strs()
            .iter()
            .any(|h| h.contains("BESKAR_LOCK_TIMEOUT")),
        "{}",
        doc.stdout
    );
    run_env(&w, "api", &["repo", "enable", "research"], &no_wait)
        .code(1)
        .err_has("another beskar process is busy");
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );

    // Questions that change nothing take no lock.
    for (cwd, args) in [
        ("api", &["status"][..]),
        ("api", &["update", "--dry-run"]),
        ("api", &["repo", "diff"]),
        (".", &["library", "list"]),
        (".", &["registry", "list"]),
        (".", &["registry", "status"]),
        (".", &["profile", "show", "coding"]),
    ] {
        run_env(&w, cwd, args, &no_wait).ok();
    }
    assert_eq!(w.read(profile), profile_before);
    assert_eq!(w.read(".beskar/registry.bsk"), registry_before);

    drop(lock);
    run_env(&w, ".", &["profile", "add", "coding", "pdf"], &no_wait).ok();
}

#[test]
fn a_second_process_waits_for_the_lock_and_says_so() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let lock = hold_lock(&w);
    let mut child = w
        .command("api", &["--json", "repo", "enable", "research"])
        .env("BESKAR_LOCK_TIMEOUT", "120")
        .spawn()
        .unwrap();
    // Even with --json, a wait is announced on standard error for whoever
    // is watching.
    let rx = watch(child.stderr.take().unwrap());
    let mut seen = String::new();
    wait_for(&rx, &mut seen, "waiting for it to finish", 1);
    assert!(seen.contains("command test holder"), "{seen}");
    assert!(child.try_wait().unwrap().is_none(), "it did not wait");
    assert!(!w.read(".beskar/registry.bsk").contains("profile: research"));
    drop(lock);
    let doc = w.doc(Run::of(child.wait_with_output().unwrap())).ok();
    assert_eq!(doc["data"]["enabled"].strs(), ["research"]);
    assert_eq!(doc.notices(), ["waiting"]);
    assert!(
        doc["notices"][0]["holder"]
            .as_str()
            .contains("command test holder")
    );
    assert!(w.read(".beskar/registry.bsk").contains("profile: research"));
}

#[test]
fn a_person_at_the_prompt_does_not_hold_the_lock() {
    let w = diverged();
    let mut child = w
        .command("api", &["update", "--on-conflict", "ask"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = watch(child.stderr.take().unwrap());
    let mut seen = String::new();
    wait_for(&rx, &mut seen, "Choice: ", 1);
    run_env(
        &w,
        ".",
        &["profile", "add", "research", "testing"],
        &[("BESKAR_LOCK_TIMEOUT", "0")],
    )
    .ok();
    child.stdin.take().unwrap().write_all(b"k\n").unwrap();
    let run = Run::of(child.wait_with_output().unwrap()).ok();
    assert!(run.stdout.contains("kept the local copy"), "{}", run.stdout);
}

#[test]
fn a_decision_is_asked_again_when_the_skill_changed_meanwhile() {
    let w = diverged();
    let mut child = w
        .command("api", &["update", "--on-conflict", "ask"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = watch(child.stderr.take().unwrap());
    let mut stdin = child.stdin.take().unwrap();
    let mut seen = String::new();
    wait_for(&rx, &mut seen, "Choice: ", 1);
    // The library moves on while the person thinks.
    w.append(&format!("{LIB}/git/SKILL.md"), "a newer library change\n");
    stdin.write_all(b"k\n").unwrap();
    wait_for(&rx, &mut seen, "Choice: ", 2);
    assert_eq!(seen.matches("Conflict:").count(), 2, "{seen}");
    stdin.write_all(b"k\n").unwrap();
    drop(stdin);
    Run::of(child.wait_with_output().unwrap()).ok();
    let doc = w.json("api", &["status"]).ok();
    let step = doc["data"]["repos"][0]["plan"]["steps"].find("skill", "git");
    assert_eq!(step["action"].as_str(), "keep_local");
    assert_eq!(
        step["kept"], step["library"],
        "the newer version was declined"
    );
}

// ----- Interrupted runs -----

/// Workspace `api` as a swap of `git` left it when killed halfway: the old
/// copy moved aside, the new one staged but not moved in.
fn interrupted_swap(w: &World) {
    let skills = "api/.agents/skills";
    fs::create_dir_all(w.path(&format!("{skills}/.beskar"))).unwrap();
    fs::rename(
        w.path(&format!("{skills}/git")),
        w.path(&format!("{skills}/.beskar/old-git-{DEAD_PID}-0")),
    )
    .unwrap();
    w.write(
        &format!("{skills}/.beskar/staging-git-{DEAD_PID}-1/SKILL.md"),
        "half-written new version\n",
    );
}

#[test]
fn an_interrupted_swap_is_undone_by_the_next_update() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let git = w.read("api/.agents/skills/git/SKILL.md");
    interrupted_swap(&w);

    let doc = w.json("api", &["status"]).ok();
    assert_eq!(doc["data"]["repos"][0]["leftovers"].as_array().len(), 2);
    let run = w.run("api", &["status"]).ok();
    let advice = run
        .stdout
        .lines()
        .find(|line| line.starts_with("An interrupted run left"))
        .unwrap_or_else(|| panic!("no advice about leftovers:\n{}", run.stdout));
    assert!(
        advice.starts_with("An interrupted run left 2 temporary"),
        "{advice}"
    );
    assert!(!advice.contains("entrys"), "{advice}");

    w.run("api", &["update"]).ok().err_has("interrupted");
    assert_eq!(w.read("api/.agents/skills/git/SKILL.md"), git);
    assert!(!w.exists("api/.agents/skills/.beskar"));
    w.run("api", &["status"])
        .ok()
        .out_has("Everything is up to date.");

    interrupted_swap(&w);
    let doc = w.json("api", &["update"]).ok();
    assert_eq!(doc.notices(), ["recovered", "recovered"]);
    let actions: Vec<&str> = doc["notices"]
        .as_array()
        .iter()
        .map(|n| n["action"].as_str())
        .collect();
    assert_eq!(actions, ["put_back", "deleted"]);
    assert_eq!(
        doc["notices"][0]["target"].as_str(),
        w.abs("api/.agents/skills/git")
    );
    assert_eq!(w.read("api/.agents/skills/git/SKILL.md"), git);
    assert!(!w.exists("api/.agents/skills/.beskar"));
    assert_eq!(doc["data"]["repos"][0]["result"].as_str(), "up_to_date");
}

#[test]
fn files_carried_into_an_unfinished_copy_go_back() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.write("api/.agents/skills/git/.git/HEAD", "ref: refs/heads/main\n");
    interrupted_swap(&w);
    // The swap had already carried the checkout into the staging copy.
    fs::rename(
        w.path(&format!(
            "api/.agents/skills/.beskar/old-git-{DEAD_PID}-0/.git"
        )),
        w.path(&format!(
            "api/.agents/skills/.beskar/staging-git-{DEAD_PID}-1/.git"
        )),
    )
    .unwrap();
    w.run("api", &["update"]).ok();
    assert_eq!(
        w.read("api/.agents/skills/git/.git/HEAD"),
        "ref: refs/heads/main\n"
    );
    assert!(!w.exists("api/.agents/skills/.beskar"));
}

#[test]
fn leftovers_that_hold_other_files_stay_and_are_reported() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    // A finished swap whose old copy still holds a checkout.
    let old = format!("api/.agents/skills/.beskar/old-git-{DEAD_PID}-0");
    w.write(&format!("{old}/SKILL.md"), "old\n");
    w.write(&format!("{old}/.git/HEAD"), "ref: refs/heads/main\n");
    w.run("api", &["update"])
        .ok()
        .err_has("warning: an earlier run was interrupted and left");
    assert_eq!(
        w.read(&format!("{old}/.git/HEAD")),
        "ref: refs/heads/main\n"
    );
    let doc = w.json("api", &["update"]).ok();
    let notice = &doc["notices"][0];
    assert_eq!(notice["action"].as_str(), "kept");
    assert!(notice["reason"].as_str().contains(".git"), "{notice}");
    w.run(".", &["doctor"]).out_has("old-git");
}

#[test]
fn leftovers_are_recovered_even_when_their_process_id_is_in_use_again() {
    // In a container Beskar can get the same process id on every run, so
    // an interrupted run's leftovers can carry the id of a live process
    // (here, this test). Only the lock says who is working, and the next
    // run holds it.
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let skills = "api/.agents/skills";
    let pid = std::process::id();
    w.write(
        &format!("{skills}/.beskar/staging-git-{pid}-0/.env"),
        "SECRET=1\n",
    );
    w.write(
        &format!("{skills}/.beskar/trash-old-{pid}-1/SKILL.md"),
        "being deleted\n",
    );
    w.write(
        ".beskar/config.bsk",
        &(w.read(".beskar/config.bsk") + "ignore: .env\n"),
    );
    let doc = w.json("api", &["update"]).ok();
    assert_eq!(doc.notices().len(), 2, "{}", doc.stdout);
    assert_eq!(w.read(&format!("{skills}/git/.env")), "SECRET=1\n");
    assert!(!w.exists(&format!("{skills}/.beskar")));
}

#[test]
fn leftovers_in_beskars_own_files_are_cleaned_up() {
    let w = World::with_library();
    let registry = w.read(".beskar/registry.bsk");
    let write = format!(".beskar/.beskar-write-registry.bsk-{DEAD_PID}-0");
    w.write(&write, "half a registry");
    let trash = format!("{LIB}/.beskar/trash-old-skill-{DEAD_PID}-0/SKILL.md");
    w.write(&trash, "being deleted\n");
    let profile = format!(".beskar/library/profiles/.beskar-write-coding.bsk-{DEAD_PID}-3");
    w.write(&profile, "skill: half\n");
    let doc = w.json(".", &["profile", "add", "research", "testing"]).ok();
    assert_eq!(doc.notices().len(), 3, "{}", doc.stdout);
    assert!(!w.exists(&write) && !w.exists(&profile));
    assert!(!w.exists(&format!("{LIB}/.beskar")));
    assert_eq!(w.read(".beskar/registry.bsk"), registry);
    let doc = w.json(".", &["library", "list"]).ok();
    assert_eq!(doc["data"].as_array().len(), 5);
}

// ----- Links -----

#[cfg(unix)]
#[test]
fn a_workspace_linked_into_the_library_is_refused() {
    use std::os::unix::fs::symlink;

    let w = World::with_library();
    // Not registered yet: `repo add` refuses.
    fs::create_dir_all(w.path("fresh")).unwrap();
    symlink(w.path(&format!("{LIB}/pdf")), w.path("fresh/.agents")).unwrap();
    w.run("fresh", &["repo", "add", "."])
        .code(1)
        .err_has("leads into the library");
    let doc = w.json("fresh", &["repo", "add", "."]).code(1);
    assert_eq!(doc["error"]["kind"].as_str(), "invalid");

    // Registered, then linked: every change refuses.
    w.run("api", &["repo", "add", "."]).ok();
    w.run("api", &["repo", "enable", "coding"]).ok();
    symlink(w.path(&format!("{LIB}/pdf")), w.path("api/.agents")).unwrap();
    let registry = w.read(".beskar/registry.bsk");
    let library_now = w.tree(".beskar/library");
    for args in [
        &["update"][..],
        &["update", "--on-conflict", "replace"],
        &["update", "--all"],
        &["registry", "update"],
        &["repo", "remove", "--purge", "--on-conflict", "replace"],
    ] {
        let run = w.run("api", args).code(1);
        assert!(
            (run.stdout.clone() + &run.stderr).contains("leads into the library"),
            "{args:?}:\n{}\n{}",
            run.stdout,
            run.stderr
        );
    }
    w.run("api", &["repo", "promote", "git", "--force"]).code(1);
    let doc = w.json("api", &["update"]).code(1);
    let error = &doc["data"]["repos"][0]["error"];
    assert_eq!(error["kind"].as_str(), "invalid");
    assert_eq!(w.tree(".beskar/library"), library_now);
    assert_eq!(w.read(".beskar/registry.bsk"), registry);
    assert_eq!(w.list(&format!("{LIB}/pdf")), ["SKILL.md"]);
}

#[cfg(unix)]
#[test]
fn restore_refuses_a_workspace_linked_into_the_library() {
    use std::os::unix::fs::symlink;

    let w = World::with_library();
    w.run("api", &["repo", "add", "."]).ok();
    w.run("api", &["repo", "enable", "coding"]).ok();
    symlink(w.path(&format!("{LIB}/pdf")), w.path("api/.agents")).unwrap();
    let library = w.tree(".beskar/library");
    let registry = w.read(".beskar/registry.bsk");
    w.run("api", &["repo", "restore", "git", "--yes"])
        .code(1)
        .err_has("leads into the library");
    assert_eq!(w.tree(".beskar/library"), library);
    assert_eq!(w.read(".beskar/registry.bsk"), registry);
}

#[cfg(unix)]
#[test]
fn a_skills_directory_that_is_the_library_or_beskars_home_is_refused() {
    use std::os::unix::fs::symlink;

    let w = World::with_library();
    w.run("a", &["repo", "add", "."]).ok();
    w.run("a", &["repo", "enable", "coding"]).ok();
    w.run("b", &["repo", "add", "."]).ok();
    w.run("b", &["repo", "enable", "research"]).ok();
    let before = w.tree(".beskar/library");
    // `.agents/skills` would be the library's own skills directory.
    symlink(w.path(".beskar/library"), w.path("a/.agents")).unwrap();
    fs::create_dir_all(w.path("b/.agents")).unwrap();
    symlink(w.path(".beskar"), w.path("b/.agents/skills")).unwrap();
    w.run("a", &["update", "--on-conflict", "replace"])
        .code(1)
        .err_has("leads into the library");
    w.run("b", &["update", "--on-conflict", "replace"])
        .code(1)
        .err_has("leads into Beskar's home directory");
    w.run(".", &["update", "--all", "--on-conflict", "replace"])
        .code(1);
    assert_eq!(w.tree(".beskar/library"), before);
    w.run(".", &["library", "list"]).ok().out_has("playwright");
}

#[cfg(unix)]
#[test]
fn links_in_a_workspace_are_never_followed_when_deleting() {
    use std::os::unix::fs::symlink;

    let w = World::with_library();
    w.write("precious/dir/file", "keep\n");
    w.write("precious/top.md", "keep too\n");
    let precious = w.tree("precious");
    w.workspace("api", &["coding", "research"]);

    // Links added to an installed copy, which is then deleted.
    symlink(
        w.path("precious/dir"),
        w.path("api/.agents/skills/pdf/data"),
    )
    .unwrap();
    symlink(
        w.path("precious/top.md"),
        w.path("api/.agents/skills/pdf/top.md"),
    )
    .unwrap();
    w.run("api", &["repo", "disable", "research"]).ok();
    w.run("api", &["update", "--on-conflict", "replace"])
        .ok()
        .out_has("-  pdf");
    assert!(!w.exists("api/.agents/skills/pdf"));
    assert_eq!(w.tree("precious"), precious);

    // A skill directory that is a link, replaced and then removed.
    fs::remove_dir_all(w.path("api/.agents/skills/git")).unwrap();
    symlink(w.path("precious"), w.path("api/.agents/skills/git")).unwrap();
    w.append(&format!("{LIB}/git/SKILL.md"), "v2\n");
    w.run("api", &["update", "--on-conflict", "replace"]).ok();
    assert!(w.path("api/.agents/skills/git").is_dir());
    assert!(w.read("api/.agents/skills/git/SKILL.md").contains("v2"));
    assert_eq!(w.tree("precious"), precious);
    fs::remove_dir_all(w.path("api/.agents/skills/git")).unwrap();
    symlink(w.path("precious"), w.path("api/.agents/skills/git")).unwrap();
    w.run(".", &["profile", "remove", "coding", "git"]).ok();
    w.run("api", &["update", "--on-conflict", "replace"]).ok();
    assert!(!w.exists("api/.agents/skills/git"));
    assert_eq!(w.tree("precious"), precious);
    w.run(
        "api",
        &["repo", "remove", "--purge", "--on-conflict", "replace"],
    )
    .ok();
    assert_eq!(w.tree("precious"), precious);
}

#[test]
fn names_cannot_reach_outside_their_directories() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    // Everything but the lock file, whose text names the last holder.
    let state = || {
        let mut tree = w.tree(".beskar");
        tree.remove("lock");
        tree
    };
    let before = state();
    for (cwd, args) in [
        (".", &["profile", "create", "../evil"][..]),
        (".", &["profile", "create", "../../evil", "git"]),
        (".", &["profile", "show", "../config"]),
        (".", &["profile", "delete", "../config", "--yes", "--force"]),
        (".", &["profile", "add", "coding", "../pdf"]),
        (".", &["library", "show", "../profiles"]),
        (
            ".",
            &["library", "remove", "../../config", "--yes", "--force"],
        ),
        (".", &["library", "remove", ".", "--yes", "--force"]),
        (
            ".",
            &["library", "add", "my-skills/git", "--name", "../evil"],
        ),
        ("api", &["repo", "enable", "../../registry"]),
        ("api", &["repo", "restore", "../git", "--yes"]),
        (
            "api",
            &["repo", "promote", "../../library/skills/git", "--force"],
        ),
        ("api", &["repo", "diff", "../git"]),
    ] {
        let run = w.run(cwd, args);
        assert_ne!(run.code, 0, "{args:?}:\n{}", run.stdout);
        assert!(
            run.stderr.contains("not a valid") || run.stderr.contains("no skill `"),
            "{args:?}:\n{}",
            run.stderr
        );
    }
    assert_eq!(state(), before);
    assert!(!w.exists("evil.bsk") && !w.exists(".beskar/evil.bsk"));
}

// ----- Arguments and output -----

#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_a_usage_error() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let w = World::with_library();
    let output = w
        .command(".", &["repo", "add"])
        .arg(OsString::from_vec(b"bad\xffname".to_vec()))
        .output()
        .unwrap();
    Run::of(output)
        .code(2)
        .err_has("is not valid UTF-8")
        .err_has("help:");
    let doc = w.json(".", &["repo", "list"]).ok();
    assert!(doc["data"].as_array().is_empty());
}

#[test]
fn a_closed_output_pipe_is_not_a_crash() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    // A diff far larger than a pipe's buffer, so writing goes on after the
    // reader is gone.
    let lines: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    w.append("api/.agents/skills/git/SKILL.md", &lines);
    for (cwd, args, read) in [
        (".", &["library", "list"][..], 0),
        (".", &["--json", "library", "list"], 0),
        ("api", &["repo", "diff"], 100),
        ("api", &["--json", "repo", "diff"], 100),
        (".", &["help"], 10),
    ] {
        let mut child = w.command(cwd, args).spawn().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = vec![0u8; read];
        stdout.read_exact(&mut buf).unwrap();
        drop(stdout);
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{args:?}: {:?}\n{stderr}",
            output.status
        );
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }

    // A change still happens, and is recorded, when nobody reads.
    w.run("api", &["repo", "enable", "research"]).ok();
    let mut child = w.command("api", &["update"]).spawn().unwrap();
    drop(child.stdout.take());
    let run = Run::of(child.wait_with_output().unwrap());
    assert!(matches!(run.code, 0 | 1), "{}", run.stderr);
    assert!(w.exists("api/.agents/skills/pdf/SKILL.md"));
    w.run("api", &["status"]).ok().out_has("✓  pdf");
}

#[cfg(unix)]
#[test]
fn output_that_cannot_be_written_fails_the_command() {
    let Ok(full) = fs::OpenOptions::new().write(true).open("/dev/full") else {
        return;
    };
    let w = World::with_library();
    let run = Run::of(
        w.command(".", &["library", "list"])
            .stdout(full.try_clone().unwrap())
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stderr.contains("cannot write the output"),
        "{}",
        run.stderr
    );

    // The work is done even so; only the report is lost.
    w.run("api", &["repo", "add", "."]).ok();
    w.run("api", &["repo", "enable", "coding"]).ok();
    let run = Run::of(
        w.command("api", &["update"])
            .stdout(full.try_clone().unwrap())
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(w.exists("api/.agents/skills/git/SKILL.md"));
}

#[cfg(unix)]
#[test]
fn a_json_document_that_cannot_be_written_fails_the_command() {
    let Ok(full) = fs::OpenOptions::new().write(true).open("/dev/full") else {
        return;
    };
    let w = World::with_library();
    let run = Run::of(
        w.command(".", &["--json", "library", "list"])
            .stdout(full)
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stderr.contains("cannot write the output"),
        "{}",
        run.stderr
    );
}

// ----- Broken files -----

#[test]
fn a_broken_profile_never_deletes_installed_skills() {
    let w = World::with_library();
    w.workspace("api", &["coding", "research"]);
    let profile = ".beskar/library/profiles/coding.bsk";
    let good = w.read(profile);
    let workspace = w.tree("api");
    let registry = w.read(".beskar/registry.bsk");
    let broken: &[(&str, &[u8])] = &[
        ("a YAML list", b"description: x\nskills:\n  - git\n"),
        ("a missing colon", b"skill git\n"),
        ("an unknown key", b"skills: git\nskills: testing\n"),
        ("a section", b"skill: git\n[section x]\nskill: testing\n"),
        ("an invalid name", b"skill: Git\n"),
        ("a duplicate", b"skill: git\nskill: git\n"),
        ("bytes that are not UTF-8", b"\xff\xfeskill: git\n"),
    ];
    let attempts: &[&[&str]] = &[
        &["update"],
        &["update", "--on-conflict", "replace"],
        &["update", "--all", "--on-conflict", "replace"],
        &["registry", "update", "--on-conflict", "replace"],
        &["--json", "update", "--on-conflict", "replace"],
    ];
    let check = |what: &str| {
        for args in attempts {
            let run = w.run("api", args);
            assert_eq!(
                run.code, 1,
                "{what}, {args:?}:\n{}\n{}",
                run.stdout, run.stderr
            );
            assert_eq!(w.tree("api"), workspace, "{what}, {args:?}");
            assert_eq!(w.read(".beskar/registry.bsk"), registry, "{what}, {args:?}");
        }
    };
    for (what, bytes) in broken {
        fs::write(w.path(profile), bytes).unwrap();
        check(what);
    }
    fs::remove_file(w.path(profile)).unwrap();
    fs::create_dir(w.path(profile)).unwrap();
    check("a directory in place of the file");
    fs::remove_dir(w.path(profile)).unwrap();
    check("a missing file");

    // A broken profile the workspace does not enable is no reason to stop.
    w.write(profile, &good);
    w.write(".beskar/library/profiles/other.bsk", "skills:\n  - git\n");
    w.run("api", &["update"]).ok();
    assert_eq!(w.tree("api"), workspace);
}

#[test]
fn a_broken_registry_or_config_never_deletes_installed_skills() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let workspace = w.tree("api");
    let registry = w.read(".beskar/registry.bsk");
    w.write(".beskar/registry.bsk", "version: 1\n[repo\ninstalled git\n");
    w.run("api", &["update", "--on-conflict", "replace"])
        .code(1)
        .err_has("registry.bsk");
    w.run(".", &["update", "--all"]).code(1);
    assert_eq!(w.tree("api"), workspace);
    w.write(".beskar/registry.bsk", &registry);

    let config = w.read(".beskar/config.bsk");
    w.write(
        ".beskar/config.bsk",
        &format!("{config}skills-dir .agents/skills\n"),
    );
    w.run("api", &["update", "--on-conflict", "replace"])
        .code(1)
        .err_has("config.bsk");
    assert_eq!(w.tree("api"), workspace);
    assert_eq!(w.read(".beskar/registry.bsk"), registry);
}

// ----- Keeping, promoting and answering through a pipe -----

#[test]
fn a_kept_copy_needs_force_to_be_promoted() {
    let w = diverged();
    w.run("api", &["update", "--on-conflict", "keep"])
        .ok()
        .out_has("kept the local copy");
    let library = w.read(&format!("{LIB}/git/SKILL.md"));
    w.run("api", &["repo", "promote", "git"])
        .code(3)
        .err_has("promoting would discard those changes")
        .err_has("--force");
    let doc = w.json("api", &["repo", "promote", "git"]).code(3);
    assert_eq!(doc["error"]["kind"].as_str(), "conflict");
    assert_eq!(w.read(&format!("{LIB}/git/SKILL.md")), library);
    w.run("api", &["repo", "promote", "git", "--force"])
        .ok()
        .out_has("Promoted git");
    let text = w.read(&format!("{LIB}/git/SKILL.md"));
    assert!(
        text.contains("local tweak") && !text.contains("library change"),
        "{text}"
    );
    let doc = w.json("api", &["status"]).ok();
    let step = doc["data"]["repos"][0]["plan"]["steps"].find("skill", "git");
    assert_eq!(step["action"].as_str(), "unchanged");
    assert!(step["kept"].is_null());
}

#[test]
fn a_kept_copy_whose_change_is_undone_takes_the_library_version() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let original = w.read("api/.agents/skills/git/SKILL.md");
    w.append("api/.agents/skills/git/SKILL.md", "local tweak\n");
    w.append(&format!("{LIB}/git/SKILL.md"), "library change\n");
    w.run("api", &["update", "--on-conflict", "keep"]).ok();
    w.write("api/.agents/skills/git/SKILL.md", &original);
    w.run("api", &["update"]).ok().out_has("~  git");
    assert!(
        w.read("api/.agents/skills/git/SKILL.md")
            .contains("library change")
    );
}

#[test]
fn answers_piped_to_an_explicit_ask() {
    let ask = ["update", "--on-conflict", "ask"];
    let local = "api/.agents/skills/git/SKILL.md";

    let w = diverged();
    let run = w.run_input("api", &ask, "k\n").ok();
    assert!(run.stdout.contains("kept the local copy"), "{}", run.stdout);
    assert!(w.read(local).contains("local tweak"));
    assert!(w.read(".beskar/registry.bsk").contains("kept: git "));

    let w = diverged();
    w.run_input("api", &ask, "l\ny\n")
        .ok()
        .out_has("replaced local changes");
    let text = w.read(local);
    assert!(
        text.contains("library change") && !text.contains("local tweak"),
        "{text}"
    );

    let w = diverged();
    w.run_input("api", &ask, "p\ny\n")
        .ok()
        .out_has("promoted to the library");
    assert!(
        w.read(&format!("{LIB}/git/SKILL.md"))
            .contains("local tweak")
    );

    let w = diverged();
    w.run_input("api", &ask, "d\nk\n")
        .ok()
        .err_has("-library change")
        .err_has("+local tweak");

    let w = diverged();
    w.run_input("api", &ask, "maybe\nk\n")
        .ok()
        .err_has("Please answer k, l, p, d, a.");

    for (answers, what) in [
        ("a\n", "abort"),
        ("l\nn\n", "declining, then the end of input"),
        ("", "no answer at all"),
    ] {
        let w = diverged();
        let before = w.tree("api");
        let registry = w.read(".beskar/registry.bsk");
        let run = w.run_input("api", &ask, answers).code(3);
        assert!(
            run.stdout.contains("nothing changed here"),
            "{what}: {}",
            run.stdout
        );
        assert_eq!(w.tree("api"), before, "{what}");
        assert_eq!(w.read(".beskar/registry.bsk"), registry, "{what}");
    }
}

#[test]
fn answers_piped_for_a_skill_no_profile_wants_any_more() {
    let w = World::with_library();
    w.workspace("api", &["coding", "research"]);
    w.append("api/.agents/skills/pdf/SKILL.md", "my notes\n");
    w.run("api", &["repo", "disable", "research"]).ok();
    w.run_input("api", &["update", "--on-conflict", "ask"], "k\n")
        .ok()
        .out_has("no longer managed");
    assert!(
        w.read("api/.agents/skills/pdf/SKILL.md")
            .contains("my notes")
    );
    w.run("api", &["status"])
        .ok()
        .out_has("not managed by Beskar");

    let w = World::with_library();
    w.workspace("api", &["coding", "research"]);
    w.append("api/.agents/skills/pdf/SKILL.md", "my notes\n");
    w.run("api", &["repo", "disable", "research"]).ok();
    w.run_input("api", &["update", "--on-conflict", "ask"], "l\ny\n")
        .ok()
        .out_has("-  pdf");
    assert!(!w.exists("api/.agents/skills/pdf"));
}

#[test]
fn registry_update_reads_piped_answers_like_update_all() {
    let w = diverged();
    w.run_input(".", &["update", "--all", "--on-conflict", "ask"], "k\n")
        .ok()
        .out_has("kept the local copy");
    let w = diverged();
    let run = w
        .run_input(".", &["registry", "update", "--on-conflict", "ask"], "k\n")
        .ok();
    assert!(run.stdout.contains("kept the local copy"), "{}", run.stdout);
}

// ----- Copies Beskar lets go of -----

#[test]
fn a_copy_with_its_own_git_is_released_not_deleted() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.write(
        "api/.agents/skills/testing/.git/HEAD",
        "ref: refs/heads/main\n",
    );
    w.run(".", &["profile", "remove", "coding", "testing"]).ok();
    w.run("api", &["update"]).ok().out_has("no longer managed");
    assert!(w.exists("api/.agents/skills/testing/SKILL.md"));
    assert!(w.exists("api/.agents/skills/testing/.git/HEAD"));
    w.run("api", &["status"])
        .ok()
        .out_has("not managed by Beskar");
    let doc = w.json("api", &["status"]).ok();
    let repo = &doc["data"]["repos"][0];
    let step = repo["plan"]["steps"].find("skill", "testing");
    assert_eq!(step["action"].as_str(), "unmanaged");
    assert!(
        repo["installed"]
            .as_array()
            .iter()
            .all(|i| i["skill"].as_str() != "testing"),
        "{repo}"
    );
    let doc = w.json("api", &["update"]).ok();
    assert_eq!(doc["data"]["repos"][0]["result"].as_str(), "up_to_date");
}

#[test]
fn a_copy_with_its_own_git_is_released_alongside_other_changes() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.write(
        "api/.agents/skills/testing/.git/HEAD",
        "ref: refs/heads/main\n",
    );
    w.run(".", &["profile", "remove", "coding", "testing"]).ok();
    w.run(".", &["profile", "add", "coding", "pdf"]).ok();
    let doc = w.json("api", &["update"]).ok();
    let outcomes = &doc["data"]["repos"][0]["outcomes"];
    assert_eq!(
        outcomes.find("skill", "testing")["done"].as_str(),
        "released"
    );
    assert_eq!(outcomes.find("skill", "pdf")["done"].as_str(), "installed");
    assert!(w.exists("api/.agents/skills/testing/.git/HEAD"));
    w.run("api", &["status"])
        .ok()
        .out_has("not managed by Beskar");
}

#[test]
fn purge_deletes_installed_skills_and_an_empty_agents_directory() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.run("api", &["repo", "remove", "--purge"])
        .ok()
        .out_has("Unregistered");
    assert!(!w.exists("api/.agents"));
    assert!(w.exists("api"));

    // Local changes stop the purge, unless a policy decides.
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.append("api/.agents/skills/git/SKILL.md", "local\n");
    w.write("api/.agents/skills/handmade/SKILL.md", "mine\n");
    w.write("api/.agents/settings.toml", "x = 1\n");
    let before = w.tree("api");
    w.run("api", &["repo", "remove", "--purge"])
        .code(3)
        .out_has("still registered");
    assert_eq!(w.tree("api"), before);
    let doc = w
        .json(
            "api",
            &["repo", "remove", "--purge", "--on-conflict", "keep"],
        )
        .ok();
    assert!(doc["data"]["unregistered"].as_bool());
    let outcomes = &doc["data"]["purge"]["outcomes"];
    assert_eq!(outcomes.find("skill", "git")["done"].as_str(), "released");
    assert_eq!(
        outcomes.find("skill", "testing")["done"].as_str(),
        "removed"
    );
    assert_eq!(w.list("api/.agents/skills"), ["git", "handmade"]);
    assert!(w.read("api/.agents/skills/git/SKILL.md").contains("local"));
    assert_eq!(w.read("api/.agents/settings.toml"), "x = 1\n");

    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.append("api/.agents/skills/git/SKILL.md", "local\n");
    w.run(
        "api",
        &["repo", "remove", "--purge", "--on-conflict", "replace"],
    )
    .ok();
    assert!(!w.exists("api/.agents"));
    w.run(".", &["repo", "list"])
        .ok()
        .out_has("No workspaces registered.");
}
