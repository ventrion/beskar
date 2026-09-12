//! `beskar doctor` — read-only diagnostics (spec §83, §136 Phase 4).
//!
//! Doctor inspects global configuration, the Library, its Git state,
//! committed-file formats, the Registry, and lock state, reporting one
//! [`Diagnostic`] per finding. It MUST NOT perform destructive repair
//! automatically (§83): findings are data; fixing them is an explicit user
//! action. Each check carries a stable snake_case identifier suitable for
//! JSON output (§130 style); UI code classifies by [`Severity`], never by
//! parsing prose (§115).

use std::collections::BTreeMap;
use std::path::Path;

use beskar_git::{GitBackend, SystemGitBackend};
use serde::{Deserialize, Serialize};

use crate::config::PlatformDirs;
use crate::library::Library;
use crate::profile::Profile;
use crate::registry::SCHEMA as REGISTRY_SCHEMA;

/// Severity of one diagnostic finding (§83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

/// One doctor finding: stable check identifier, severity, human message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable check identifier, e.g. `duplicate_skills` (§130 style).
    pub check: String,
    pub severity: Severity,
    pub message: String,
}

/// The complete doctor report (§83).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<Diagnostic>,
}

impl DoctorReport {
    /// Whether any check found an error (the CLI maps this to a non-zero
    /// exit; warnings alone do not fail, §83).
    pub fn has_errors(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.severity == Severity::Error)
    }

    /// Whether any check produced a warning.
    pub fn has_warnings(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.severity == Severity::Warning)
    }
}

/// Runs every doctor check (§83). `start` is the directory library
/// discovery starts from (respecting `BESKAR_LIBRARY`, §86). No check
/// mutates anything and no network access occurs.
pub fn examine(dirs: &PlatformDirs, start: &Path) -> DoctorReport {
    let backend = SystemGitBackend;
    let mut checks = Vec::new();

    check_git_executable(&mut checks);
    check_configuration(dirs, &mut checks);

    let library = match Library::discover(start) {
        Ok(library) => {
            checks.push(Diagnostic {
                check: "library".to_owned(),
                severity: Severity::Ok,
                message: format!(
                    "library at {} (id {})",
                    library.root().display(),
                    library.config().library_id
                ),
            });
            Some(library)
        }
        Err(err) => {
            checks.push(Diagnostic {
                check: "library".to_owned(),
                severity: Severity::Error,
                message: err.to_string(),
            });
            None
        }
    };

    let Some(library) = library else {
        // Registry and lock checks remain meaningful without a library.
        check_registry(dirs, &mut checks);
        check_locks(dirs, &mut checks);
        return DoctorReport { checks };
    };

    check_git_repository(&backend, &library, &mut checks);
    check_config_file(&library, &mut checks);
    check_catalog(&library, &mut checks);
    check_skills(&library, &mut checks);
    check_profiles(&library, &mut checks);
    check_profile_skills(&library, &mut checks);
    check_remote(&backend, &library, &mut checks);

    check_registry(dirs, &mut checks);
    check_installations(dirs, &backend, &library, &mut checks);
    check_locks(dirs, &mut checks);
    DoctorReport { checks }
}

/// The system Git executable exists and runs (§83 "Git executable").
fn check_git_executable(checks: &mut Vec<Diagnostic>) {
    match beskar_git::git_version() {
        Ok(version) => checks.push(Diagnostic {
            check: "git_executable".to_owned(),
            severity: Severity::Ok,
            message: version,
        }),
        Err(err) => checks.push(Diagnostic {
            check: "git_executable".to_owned(),
            severity: Severity::Error,
            message: err.to_string(),
        }),
    }
}

/// Environment overrides and platform directories (§83 "global
/// configuration", §86).
fn check_configuration(dirs: &PlatformDirs, checks: &mut Vec<Diagnostic>) {
    let home_override = std::env::var_os("BESKAR_HOME").is_some();
    let library_override =
        std::env::var_os("BESKAR_LIBRARY").map(|value| value.to_string_lossy().into_owned());
    checks.push(Diagnostic {
        check: "configuration".to_owned(),
        severity: Severity::Ok,
        message: match (&home_override, &library_override) {
            (true, Some(library)) => {
                format!("BESKAR_HOME set; BESKAR_LIBRARY={library}")
            }
            (true, None) => "BESKAR_HOME set".to_owned(),
            (false, Some(library)) => {
                format!("BESKAR_LIBRARY={library}")
            }
            (false, None) => format!(
                "platform defaults: data {}, state {}",
                dirs.data_dir.display(),
                dirs.state_dir.display()
            ),
        },
    });
    if let Err(err) = std::fs::create_dir_all(&dirs.data_dir) {
        checks.push(Diagnostic {
            check: "configuration".to_owned(),
            severity: Severity::Error,
            message: format!(
                "data directory {} is not writable: {err}",
                dirs.data_dir.display()
            ),
        });
    }
}

/// Git validity of the Library repository (§83 "Git validity").
fn check_git_repository(backend: &dyn GitBackend, library: &Library, checks: &mut Vec<Diagnostic>) {
    let root = library.root();
    match backend.status(root) {
        Ok(status) => {
            let branch = status.branch.as_deref().unwrap_or("(detached HEAD)");
            checks.push(Diagnostic {
                check: "git_repository".to_owned(),
                severity: Severity::Ok,
                message: format!("valid git repository on branch {branch}"),
            });
        }
        Err(err) => checks.push(Diagnostic {
            check: "git_repository".to_owned(),
            severity: Severity::Error,
            message: format!("{} is not a usable git repository: {err}", root.display()),
        }),
    }
}

/// `beskar.toml` presence and schema (§83; unsupported schemas fail closed
/// in `Library::open_at`, so reaching here means the schema is supported).
fn check_config_file(library: &Library, checks: &mut Vec<Diagnostic>) {
    checks.push(Diagnostic {
        check: "beskar_toml".to_owned(),
        severity: Severity::Ok,
        message: format!(
            "schema {} with default_ref {:?}",
            library.config().schema,
            library.config().default_ref
        ),
    });
}

/// `catalog.toml` parse and schema (§83). Diagnoses the working tree.
fn check_catalog(library: &Library, checks: &mut Vec<Diagnostic>) {
    let text = match std::fs::read_to_string(library.catalog_path()) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // An absent catalog is fine (§14: metadata is optional).
            checks.push(Diagnostic {
                check: "catalog".to_owned(),
                severity: Severity::Ok,
                message: "no catalog.toml in the working tree (optional, §14)".to_owned(),
            });
            return;
        }
        Err(err) => {
            checks.push(Diagnostic {
                check: "catalog".to_owned(),
                severity: Severity::Error,
                message: format!("cannot read catalog.toml: {err}"),
            });
            return;
        }
    };
    match crate::catalog::Catalog::parse_toml(&text) {
        Ok(catalog) => checks.push(Diagnostic {
            check: "catalog".to_owned(),
            severity: Severity::Ok,
            message: format!("{} catalog entries", catalog.skills.len()),
        }),
        Err(err) => checks.push(Diagnostic {
            check: "catalog".to_owned(),
            severity: Severity::Error,
            message: err.to_string(),
        }),
    }
}

/// Working-tree skills: duplicates, invalid SKILL.md, symlinks/special
/// objects, leaf mismatches (§83 "duplicate Skills", "invalid SKILL.md").
fn check_skills(library: &Library, checks: &mut Vec<Diagnostic>) {
    let (_, problems) = match crate::editing::collect_skill_names_tolerant(&library.skills_dir()) {
        Ok(collected) => collected,
        Err(err) => {
            checks.push(Diagnostic {
                check: "skills".to_owned(),
                severity: Severity::Error,
                message: err.to_string(),
            });
            return;
        }
    };
    // Also catch library-scan-level problems the tolerant walk does not see
    // (nested roots, bucket validity, duplicate profile names) by running
    // the strict scan.
    let scan_result = library.scan();
    if problems.is_empty()
        && let Ok(scan) = &scan_result
    {
        checks.push(Diagnostic {
            check: "skills".to_owned(),
            severity: Severity::Ok,
            message: format!("{} skills, all valid", scan.skills.len()),
        });
        return;
    }
    for problem in problems {
        checks.push(Diagnostic {
            check: "invalid_skill_md".to_owned(),
            severity: Severity::Error,
            message: problem,
        });
    }
    if let Err(err) = scan_result {
        checks.push(Diagnostic {
            check: "skills".to_owned(),
            severity: Severity::Error,
            message: err.to_string(),
        });
    }
}

/// Working-tree profiles: parse failures, duplicate names/IDs (§83
/// "duplicate Profiles").
fn check_profiles(library: &Library, checks: &mut Vec<Diagnostic>) {
    let profiles = match crate::editing::collect_profiles_tolerant(&library.profiles_dir()) {
        Ok(profiles) => profiles,
        Err(err) => {
            checks.push(Diagnostic {
                check: "profiles".to_owned(),
                severity: Severity::Error,
                message: err.to_string(),
            });
            return;
        }
    };
    let mut parsed: Vec<Profile> = Vec::new();
    let mut problems = 0;
    for (result, file) in &profiles {
        match result {
            Ok(profile) => parsed.push(profile.clone()),
            Err(message) => {
                problems += 1;
                checks.push(Diagnostic {
                    check: "invalid_profile".to_owned(),
                    severity: Severity::Error,
                    message: format!("{file}: {message}"),
                });
            }
        }
    }
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    let mut by_id: BTreeMap<String, String> = BTreeMap::new();
    for profile in &parsed {
        if let Some(existing) = by_id.insert(profile.id.to_string(), profile.name.clone()) {
            checks.push(Diagnostic {
                check: "duplicate_profiles".to_owned(),
                severity: Severity::Error,
                message: format!(
                    "profile id {} is used by both {existing:?} and {:?} (§15)",
                    profile.id, profile.name
                ),
            });
            problems += 1;
        }
        let folded = profile.name.to_lowercase();
        if let Some(existing) = by_name.insert(folded, profile.name.clone()) {
            checks.push(Diagnostic {
                check: "duplicate_profiles".to_owned(),
                severity: Severity::Error,
                message: format!(
                    "profile names {existing:?} and {:?} collide \
                     case-insensitively (§15)",
                    profile.name
                ),
            });
            problems += 1;
        }
    }
    if problems == 0 {
        checks.push(Diagnostic {
            check: "profiles".to_owned(),
            severity: Severity::Ok,
            message: format!("{} profiles, all valid", parsed.len()),
        });
    }
}

/// Skills referenced by profiles but absent from the working tree (§83
/// "missing Profile Skills") — a warning: gap installations cannot fully
/// converge but the library itself is well-formed (§58, §4).
fn check_profile_skills(library: &Library, checks: &mut Vec<Diagnostic>) {
    let Ok((skill_names, _)) = crate::editing::collect_skill_names_tolerant(&library.skills_dir())
    else {
        return; // already reported by check_skills
    };
    let Ok(profiles) = crate::editing::collect_profiles_tolerant(&library.profiles_dir()) else {
        return; // already reported by check_profiles
    };
    for (result, _) in profiles {
        let Ok(profile) = result else { continue };
        let missing: Vec<String> = profile
            .skills
            .iter()
            .filter(|skill| !skill_names.contains(skill))
            .map(|skill| skill.to_string())
            .collect();
        if !missing.is_empty() {
            checks.push(Diagnostic {
                check: "missing_profile_skills".to_owned(),
                severity: Severity::Warning,
                message: format!(
                    "profile {:?} references missing skills: {} (§83; \
                     installations requiring them classify as gap, §58)",
                    profile.name,
                    missing.join(", ")
                ),
            });
        }
    }
}

/// Remote configuration (§83 "remote configuration"). No network access —
/// only the configured URL is inspected (§30, §67).
fn check_remote(backend: &dyn GitBackend, library: &Library, checks: &mut Vec<Diagnostic>) {
    match backend.remote_url(library.root(), "origin") {
        Ok(Some(url)) => checks.push(Diagnostic {
            check: "remote".to_owned(),
            severity: Severity::Ok,
            message: format!("origin = {url}"),
        }),
        Ok(None) => checks.push(Diagnostic {
            check: "remote".to_owned(),
            severity: Severity::Warning,
            message: "no origin remote configured; fetch/push unavailable \
                      until one is added (§83)"
                .to_owned(),
        }),
        Err(err) => checks.push(Diagnostic {
            check: "remote".to_owned(),
            severity: Severity::Warning,
            message: err.to_string(),
        }),
    }
}

/// Registry parse validity and schema (§83 "Registry parse validity").
fn check_registry(dirs: &PlatformDirs, checks: &mut Vec<Diagnostic>) {
    let store = crate::registry::RegistryStore::from_dirs(dirs);
    match store.load() {
        Ok(registry) => checks.push(Diagnostic {
            check: "registry".to_owned(),
            severity: Severity::Ok,
            message: format!(
                "valid registry at {} with {} installation(s), schema \
                 {REGISTRY_SCHEMA}",
                store.path().display(),
                registry.installations.len()
            ),
        }),
        Err(err) => checks.push(Diagnostic {
            check: "registry".to_owned(),
            severity: Severity::Error,
            message: err.to_string(),
        }),
    }
}

/// Installation sanity: duplicate `(workspace, target)` pairs, missing
/// workspaces, missing targets, missing attached profiles, invalid source
/// refs (§83).
fn check_installations(
    dirs: &PlatformDirs,
    backend: &dyn GitBackend,
    library: &Library,
    checks: &mut Vec<Diagnostic>,
) {
    let Ok(registry) = crate::registry::RegistryStore::from_dirs(dirs).load() else {
        return; // already reported by check_registry
    };
    let library_profiles: Vec<crate::ids::ProfileId> = match library.scan() {
        Ok(scan) => scan.profiles.iter().map(|profile| profile.id).collect(),
        // The working tree may be broken; fall back to the committed HEAD.
        Err(_) => backend
            .resolve_ref(library.root(), "HEAD")
            .ok()
            .and_then(|head| crate::library::resolve_revision(backend, library, &head).ok())
            .map(|resolved| resolved.profiles.keys().copied().collect())
            .unwrap_or_default(),
    };
    let mut seen: BTreeMap<(String, String), crate::ids::InstallationId> = BTreeMap::new();
    for installation in &registry.installations {
        let key = (
            installation.workspace.display().to_string(),
            installation.target.clone(),
        );
        if let Some(existing) = seen.insert(key.clone(), installation.id) {
            checks.push(Diagnostic {
                check: "duplicate_targets".to_owned(),
                severity: Severity::Error,
                message: format!(
                    "installations {existing} and {} both own ({}, {}) (§26)",
                    installation.id, key.0, key.1
                ),
            });
        }
        if !installation.workspace.is_dir() {
            checks.push(Diagnostic {
                check: "missing_workspaces".to_owned(),
                severity: Severity::Warning,
                message: format!(
                    "workspace {} of installation {} does not exist (§83)",
                    installation.workspace.display(),
                    installation.id
                ),
            });
            // Without the workspace the target cannot exist either; the
            // missing-workspace finding already covers this pair.
            continue;
        }
        if !installation
            .workspace
            .join(crate::paths::to_native_path(&installation.target))
            .is_dir()
        {
            checks.push(Diagnostic {
                check: "missing_targets".to_owned(),
                severity: Severity::Warning,
                message: format!(
                    "target {:?} of installation {} does not exist (§83)",
                    installation.target, installation.id
                ),
            });
        }
        for attachment in &installation.profiles {
            if !library_profiles.contains(&attachment.id) {
                checks.push(Diagnostic {
                    check: "missing_attached_profiles".to_owned(),
                    severity: Severity::Warning,
                    message: format!(
                        "installation {} attaches profile {:?} ({}), which the \
                         library does not define — protected until explicitly \
                         detached (§39, §83)",
                        installation.id, attachment.name, attachment.id
                    ),
                });
            }
        }
        if backend
            .resolve_ref(library.root(), &installation.source_ref)
            .is_err()
        {
            checks.push(Diagnostic {
                check: "invalid_source_refs".to_owned(),
                severity: Severity::Warning,
                message: format!(
                    "installation {} source ref {:?} does not resolve locally \
                     — fetch first if it is a remote branch (§83)",
                    installation.id, installation.source_ref
                ),
            });
        }
    }
}

/// Stale locks where safely detectable (§83, §88): a lock file nobody
/// holds is a leftover; Beskar reports it but never removes locks blindly.
fn check_locks(dirs: &PlatformDirs, checks: &mut Vec<Diagnostic>) {
    let mut lock_files: Vec<std::path::PathBuf> = vec![
        dirs.state_dir.join("library.lock"),
        dirs.data_dir.join("registry.lock"),
        dirs.state_dir.join("targets"),
    ];
    if let Ok(entries) = std::fs::read_dir(dirs.state_dir.join("targets")) {
        for entry in entries.flatten() {
            lock_files.push(entry.path());
        }
    }
    let mut found_any = false;
    let mut stale = 0;
    for path in &lock_files {
        if path.is_dir() {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        found_any = true;
        // Try to take the advisory lock: success proves nobody holds it, so
        // the file is a leftover (safe to report; doctor never removes it).
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            match file.try_lock() {
                Ok(()) => {
                    stale += 1;
                    checks.push(Diagnostic {
                        check: "stale_locks".to_owned(),
                        severity: Severity::Warning,
                        message: format!(
                            "lock file {} exists but is not held by any \
                             process; it is safe to remove manually if no \
                             Beskar command is running (§88)",
                            path.display()
                        ),
                    });
                    // Release immediately: doctor holds no locks.
                    drop(file);
                }
                Err(_) => {
                    // Actively held: healthy concurrency, nothing to report.
                }
            }
        }
    }
    if !found_any || stale == 0 {
        checks.push(Diagnostic {
            check: "stale_locks".to_owned(),
            severity: Severity::Ok,
            message: "no stale lock files detected".to_owned(),
        });
    }
}
