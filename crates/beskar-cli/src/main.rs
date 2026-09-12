//! beskar — Better Skill Arrangement (spec §1, §111).
//!
//! The CLI is the normative behavioral interface: it owns the clap grammar,
//! argument validation, human/JSON rendering, and exit-code mapping only.
//! All behavior lives in beskar-core / beskar-git (§105, §107) — this shell
//! never re-implements domain logic and never classifies errors by parsing
//! text (§115).

mod json_out;
mod render;

use std::path::PathBuf;
use std::process::ExitCode;

use beskar_core::Error;
use beskar_core::editing::{IngestRequest, LibraryEditor, SkillFilter, SkillPivot, SkillSort};
use beskar_core::lifecycle::{
    AddRequest, DetachRequest, Lifecycle, RefSetRequest, UnregisterRequest, UpdateAllRequest,
    UpdateOneRequest,
};
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::Adapter;
use clap::{ArgAction, CommandFactory, Parser, Subcommand, ValueEnum};

/// Program exit codes (spec §94). The full contract is defined here so all
/// phases map consistently.
mod exit_codes {
    pub const SUCCESS: u8 = 0;
    pub const FAILURE: u8 = 1;
    /// clap already exits with this code for grammar usage errors.
    pub const USAGE: u8 = 2;
    pub const ACTION_REQUIRED: u8 = 3;
    /// Partial success of `update --all --best-effort` (§48).
    pub const PARTIAL_SUCCESS: u8 = 4;
    pub const INVALID_CONFIG: u8 = 5;
    pub const GIT_FAILURE: u8 = 6;
    pub const LOCKED: u8 = 7;
    pub const FS_STATE: u8 = 8;
}

/// Beskar — Better Skill Arrangement: skill library and installation manager
/// for Agent Skills.
#[derive(Debug, Parser)]
#[command(name = "beskar", version, about, disable_help_subcommand = true)]
struct Cli {
    /// Increase verbosity (-v: info, -vv: debug). RUST_LOG overrides (§116).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum AdapterArg {
    /// `.agents/skills` (default).
    Agents,
    /// `.claude/skills`.
    Claude,
    /// Requires an explicit `--target` (§24).
    Custom,
}

impl From<AdapterArg> for Adapter {
    fn from(value: AdapterArg) -> Self {
        match value {
            AdapterArg::Agents => Adapter::Agents,
            AdapterArg::Claude => Adapter::Claude,
            AdapterArg::Custom => Adapter::Custom,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Attach a profile to a workspace installation and reconcile the
    /// resulting skill union (spec §20-§24).
    Add {
        /// Profile name (or immutable ID).
        profile: String,
        /// Workspace directory (must exist).
        workspace: PathBuf,
        /// Workspace-relative target directory (overrides --adapter).
        #[arg(long)]
        target: Option<String>,
        /// Adapter selecting a conventional target (§24).
        #[arg(long)]
        adapter: Option<AdapterArg>,
        /// Source ref (default: beskar.toml `default_ref`, §18; must match
        /// the installation's existing ref, §23).
        #[arg(long = "ref")]
        reference: Option<String>,
        /// Explicit consent to overwrite locally modified managed files (§47).
        #[arg(long)]
        force: bool,
        /// Explicit consent to replace unmanaged same-name directories (§49).
        #[arg(long)]
        replace_unmanaged: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Detach a profile from a workspace installation and reconcile the
    /// remaining profiles (spec §52-§54, §40).
    Remove {
        /// Profile by name (current or last-known) or ID (§40).
        profile: String,
        /// Workspace directory.
        workspace: PathBuf,
        /// Workspace-relative target directory.
        #[arg(long)]
        target: Option<String>,
        /// Explicit consent to retire modified managed files (§51).
        #[arg(long)]
        force: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Remove an installation record from the registry (spec §55).
    Unregister {
        /// Workspace directory.
        workspace: PathBuf,
        /// Workspace-relative target directory.
        #[arg(long)]
        target: Option<String>,
        /// Remove only the registry record; never touch target contents.
        #[arg(long = "keep-files")]
        keep_files: bool,
        /// Explicit consent to retire modified managed files (§51).
        #[arg(long)]
        force: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Installation source-ref operations (spec §56).
    Ref {
        #[command(subcommand)]
        action: RefAction,
    },
    /// Explain why a skill is installed (spec §43).
    Why {
        /// Skill name.
        skill: String,
        /// Workspace directory (default: the current directory).
        workspace: Option<PathBuf>,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Read-only installation status; performs no writes and no network
    /// access (spec §41-§42).
    Status {
        /// Workspace directory (default: the current directory unless --all).
        workspace: Option<PathBuf>,
        /// Report every registered installation.
        #[arg(long)]
        all: bool,
        /// Workspace-relative target directory.
        #[arg(long)]
        target: Option<String>,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Converge installations toward their current desired state using only
    /// locally available library refs — never fetches (spec §44-§48).
    Update {
        /// Workspace directory (required unless --all).
        workspace: Option<PathBuf>,
        /// Update every registered installation (§48).
        #[arg(long)]
        all: bool,
        /// Workspace-relative target directory.
        #[arg(long)]
        target: Option<String>,
        /// Explicit consent to overwrite locally modified managed files (§47).
        #[arg(long)]
        force: bool,
        /// Explicit consent to replace unmanaged same-name directories (§49).
        #[arg(long)]
        replace_unmanaged: bool,
        /// Continue with safe installations, skipping blocked ones; the exit
        /// code signals partial success (§48).
        #[arg(long)]
        best_effort: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Ingest a skill (or, with --recursive, a batch of skills) into the
    /// library under one bucket (spec §70, §71).
    Ingest {
        /// Source directory containing a `SKILL.md` (the skill root, or a
        /// tree of skill roots with --recursive).
        path: PathBuf,
        /// Bucket beneath `skills/` for the ingested skills (§13).
        #[arg(long)]
        bucket: String,
        /// Explicit consent to replace an existing same-name skill (§71).
        #[arg(long)]
        replace: bool,
        /// Discover every skill root beneath the source path (§71).
        #[arg(long)]
        recursive: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Library skill operations (spec §69, §73-§75, §80).
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Library profile operations (spec §69, §76, §77).
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Library-level status and branch helpers (spec §81, §82).
    Library {
        #[command(subcommand)]
        action: LibraryAction,
    },
    /// Read-only diagnostics over configuration, library, and registry —
    /// never repairs automatically (spec §83).
    Doctor {
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Launch the interactive terminal UI (spec §95).
    Tui,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SortArg {
    Name,
    Bucket,
    Rank,
    Recent,
}

impl From<SortArg> for SkillSort {
    fn from(value: SortArg) -> Self {
        match value {
            SortArg::Name => SkillSort::Name,
            SortArg::Bucket => SkillSort::Bucket,
            SortArg::Rank => SkillSort::Rank,
            SortArg::Recent => SkillSort::Recent,
        }
    }
}

#[derive(Debug, Subcommand)]
enum SkillAction {
    /// List library skills with filters and sort modes (spec §80).
    List {
        /// Only skills beneath this bucket (prefix match).
        #[arg(long)]
        bucket: Option<String>,
        /// Only skills carrying this catalog tag.
        #[arg(long)]
        tag: Option<String>,
        /// Only skills required by this profile.
        #[arg(long)]
        profile: Option<String>,
        /// Lexical search over skill names and descriptions.
        #[arg(long)]
        query: Option<String>,
        /// Sort mode (§80).
        #[arg(long, value_enum)]
        sort: Option<SortArg>,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Show one skill's metadata, membership, and files (spec §69).
    Show {
        skill: String,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Move a skill to another bucket; identity is untouched (spec §73).
    Move {
        skill: String,
        /// Destination bucket (§13).
        bucket: String,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Rename a skill: an atomic identity migration (spec §74).
    Rename {
        old_name: String,
        new_name: String,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Remove a skill from the library (spec §75).
    Remove {
        skill: String,
        /// Remove the skill from every profile referencing it, in the same
        /// commit (§75).
        #[arg(long)]
        cascade: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Add or remove catalog tags (spec §14, §80).
    Tag {
        skill: String,
        /// Tags to add.
        #[arg(value_name = "TAG")]
        tags: Vec<String>,
        /// Tags to remove.
        #[arg(long, value_name = "TAG")]
        remove: Vec<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Set or clear the catalog display rank (spec §14, §80).
    Rank {
        skill: String,
        /// New rank; omit together with --clear is an error.
        rank: Option<i64>,
        /// Clear the rank instead of setting it.
        #[arg(long, conflicts_with = "rank")]
        clear: bool,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProfileAction {
    /// List library profiles (spec §76).
    List {
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Show one profile and the local installations attaching it.
    Show {
        profile: String,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Create an empty profile (spec §76; a fresh immutable UUID).
    Create {
        name: String,
        /// Human-readable description.
        #[arg(long)]
        description: Option<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Delete a profile from the library (spec §77: distinct from detach).
    Delete {
        name: String,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Rename a profile, keeping its immutable UUID (spec §76).
    Rename {
        old_name: String,
        new_name: String,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Append skills to a profile in the given order (spec §76).
    Add {
        profile: String,
        #[arg(value_name = "SKILL")]
        skills: Vec<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Remove skills from a profile (spec §76).
    Remove {
        profile: String,
        #[arg(value_name = "SKILL")]
        skills: Vec<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Move one skill before/after another in the profile order (§16, §76).
    Move {
        profile: String,
        skill: String,
        /// Place the skill directly before this pivot skill.
        #[arg(long, value_name = "SKILL")]
        before: Option<String>,
        /// Place the skill directly after this pivot skill.
        #[arg(long, value_name = "SKILL", conflicts_with = "before")]
        after: Option<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Validate one profile or all of them (spec §76).
    Validate {
        /// Profile to validate; omit to validate every profile.
        profile: Option<String>,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum LibraryAction {
    /// Local library state: branch, HEAD, dirty files, remote, refs used
    /// by registered installations — no network access (spec §81).
    Status {
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// List local branches with their upstreams (spec §82).
    Branch {
        #[command(subcommand)]
        action: Option<BranchAction>,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
    /// Switch the library working tree to another branch (spec §82).
    Switch {
        name: String,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum BranchAction {
    /// Create a branch at HEAD and switch to it (spec §82).
    Create {
        name: String,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RefAction {
    /// Change the source ref of an installation; all attached profiles move
    /// together (spec §56, §135.17).
    Set {
        /// Workspace directory.
        workspace: PathBuf,
        /// The new source ref (branch, tag, or commit).
        new_ref: String,
        /// Workspace-relative target directory.
        #[arg(long)]
        target: Option<String>,
        /// Plan and validate without writing anything (§91).
        #[arg(long)]
        dry_run: bool,
        /// Emit only structured JSON on stdout (§92, §130).
        #[arg(long)]
        json: bool,
    },
}

/// Maps typed core errors to exit codes (spec §94, §115). UI shells must
/// classify errors by type, never by parsing text.
fn exit_code(err: &Error) -> ExitCode {
    let code = match err {
        Error::Config(_) | Error::Schema(_) => exit_codes::INVALID_CONFIG,
        Error::Validation(_) => exit_codes::FAILURE,
        Error::PathSafety(_) => exit_codes::FS_STATE,
        Error::Library(_) | Error::Profile(_) | Error::ProfileAttachment(_) => exit_codes::FAILURE,
        Error::Registry(_) => exit_codes::FS_STATE,
        Error::DriftConflict(_) => exit_codes::ACTION_REQUIRED,
        Error::Lock(_) => exit_codes::LOCKED,
        Error::Git(_) | Error::RemoteAuth(_) => exit_codes::GIT_FAILURE,
        Error::Io(_) => exit_codes::FS_STATE,
        Error::UnsupportedState(_) => exit_codes::FAILURE,
    };
    ExitCode::from(code)
}

/// Per-command rendering context: the subcommand name (for JSON envelopes)
/// and whether stdout must carry only JSON (§92).
struct Ctx {
    command: &'static str,
    json: bool,
}

impl Ctx {
    /// Reports a typed core error and produces its exit code (§94).
    fn fail(&self, err: &Error) -> ExitCode {
        if self.json {
            println!("{}", json_out::error_envelope(self.command, err));
        } else {
            eprintln!("error: {err}");
        }
        exit_code(err)
    }

    /// Reports a semantic usage conflict (distinct from clap grammar errors,
    /// which exit 2 natively).
    fn usage(&self, message: impl std::fmt::Display) -> ExitCode {
        eprintln!("error: {message}");
        ExitCode::from(exit_codes::USAGE)
    }

    /// Opens the lifecycle session (§86: BESKAR_HOME/BESKAR_LIBRARY).
    fn lifecycle(&self) -> Result<Lifecycle, ExitCode> {
        Lifecycle::from_env().map_err(|err| self.fail(&err))
    }

    /// Opens the library-editing session (§86).
    fn editor(&self) -> Result<LibraryEditor, ExitCode> {
        LibraryEditor::from_env().map_err(|err| self.fail(&err))
    }

    /// The exit code for a finished mutation outcome: blocked plans demand
    /// action (§94 exit 3); otherwise success.
    fn done(&self, outcome: &beskar_core::lifecycle::OperationOutcome, dry_run: bool) -> ExitCode {
        if self.json {
            println!("{}", json_out::operation(self.command, outcome, dry_run));
        } else {
            render::operation(self.command, outcome, dry_run);
        }
        if outcome.plan.is_blocked() {
            ExitCode::from(exit_codes::ACTION_REQUIRED)
        } else {
            ExitCode::from(exit_codes::SUCCESS)
        }
    }
}

fn options(force: bool, replace_unmanaged: bool) -> ReconcileOptions {
    ReconcileOptions {
        force,
        replace_unmanaged,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        // No subcommand: print help without treating it as an error (§4:
        // safer, non-destructive default).
        None => {
            let _ = Cli::command().print_help();
            ExitCode::from(exit_codes::SUCCESS)
        }
        Some(Command::Tui) => match beskar_tui::run() {
            Ok(()) => ExitCode::from(exit_codes::SUCCESS),
            Err(err) => {
                eprintln!("error: {err}");
                exit_code(&err)
            }
        },
        Some(command) => dispatch(command),
    }
}

fn dispatch(command: Command) -> ExitCode {
    match command {
        Command::Add {
            profile,
            workspace,
            target,
            adapter,
            reference,
            force,
            replace_unmanaged,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "add",
                json,
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            let request = AddRequest {
                workspace: &workspace,
                profile: &profile,
                target: target.as_deref(),
                adapter: adapter.map(Adapter::from),
                source_ref: reference.as_deref(),
                options: options(force, replace_unmanaged),
                dry_run,
            };
            match lifecycle.add(request) {
                Ok(outcome) => ctx.done(&outcome, dry_run),
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Remove {
            profile,
            workspace,
            target,
            force,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "remove",
                json,
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            let request = DetachRequest {
                workspace: &workspace,
                profile: &profile,
                target: target.as_deref(),
                force,
                dry_run,
            };
            match lifecycle.remove(request) {
                Ok(outcome) => ctx.done(&outcome, dry_run),
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Unregister {
            workspace,
            target,
            keep_files,
            force,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "unregister",
                json,
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            let request = UnregisterRequest {
                workspace: &workspace,
                target: target.as_deref(),
                keep_files,
                force,
                dry_run,
            };
            match lifecycle.unregister(request) {
                Ok(outcome) => ctx.done(&outcome, dry_run),
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Ref {
            action:
                RefAction::Set {
                    workspace,
                    new_ref,
                    target,
                    dry_run,
                    json,
                },
        } => {
            let ctx = Ctx {
                command: "ref set",
                json,
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            let request = RefSetRequest {
                workspace: &workspace,
                target: target.as_deref(),
                new_ref: &new_ref,
                dry_run,
            };
            match lifecycle.ref_set(request) {
                Ok(outcome) => ctx.done(&outcome, dry_run),
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Why {
            skill,
            workspace,
            json,
        } => {
            let ctx = Ctx {
                command: "why",
                json,
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            match lifecycle.why(&skill, workspace.as_deref()) {
                Ok(answers) => {
                    if json {
                        println!("{}", json_out::why("why", &answers));
                    } else {
                        for answer in &answers {
                            render::why(answer);
                            println!();
                        }
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Status {
            workspace,
            all,
            target,
            json,
        } => {
            let ctx = Ctx {
                command: "status",
                json,
            };
            if all && workspace.is_some() {
                return ctx.usage("choose either a workspace or --all, not both");
            }
            let workspace = match workspace {
                Some(workspace) => Some(workspace),
                None if all => None,
                None => Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            };
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            match lifecycle.status(workspace.as_deref(), target.as_deref()) {
                Ok(reports) => {
                    if reports.is_empty() && !json {
                        println!("No registered installations.");
                    } else if json {
                        let (document, _) = json_out::status("status", &reports);
                        println!("{document}");
                    } else {
                        for report in &reports {
                            render::status(report);
                            println!();
                        }
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Update {
            workspace,
            all,
            target,
            force,
            replace_unmanaged,
            best_effort,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "update",
                json,
            };
            if all && workspace.is_some() {
                return ctx.usage("choose either a workspace or --all, not both");
            }
            let lifecycle = match ctx.lifecycle() {
                Ok(l) => l,
                Err(code) => return code,
            };
            if all {
                let request = UpdateAllRequest {
                    options: options(force, replace_unmanaged),
                    best_effort,
                    dry_run,
                };
                return match lifecycle.update_all(request) {
                    Ok(outcome) => {
                        if json {
                            println!("{}", json_out::update_all("update", &outcome));
                        } else {
                            render::update_all(&outcome);
                        }
                        // §94/§48: partial success carries its own exit code.
                        let code = match outcome.exit_code() {
                            1 => exit_codes::FAILURE,
                            3 => exit_codes::ACTION_REQUIRED,
                            4 => exit_codes::PARTIAL_SUCCESS,
                            _ => exit_codes::SUCCESS,
                        };
                        ExitCode::from(code)
                    }
                    Err(err) => ctx.fail(&err),
                };
            }
            let Some(workspace) = workspace else {
                return ctx.usage("update needs a workspace or --all");
            };
            let request = UpdateOneRequest {
                workspace: &workspace,
                target: target.as_deref(),
                options: options(force, replace_unmanaged),
                dry_run,
            };
            match lifecycle.update_one(request) {
                Ok(outcome) => ctx.done(&outcome, dry_run),
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Ingest {
            path,
            bucket,
            replace,
            recursive,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "ingest",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.ingest(IngestRequest {
                source: &path,
                bucket: &bucket,
                replace,
                recursive,
                dry_run,
            }) {
                Ok(outcome) => {
                    if json {
                        println!("{}", json_out::library_outcome("ingest", &outcome, dry_run));
                    } else {
                        render::library_outcome("ingest", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        Command::Skill { action } => dispatch_skill(action),
        Command::Profile { action } => dispatch_profile(action),
        Command::Library { action } => dispatch_library(action),
        Command::Doctor { json } => {
            let dirs = beskar_core::config::PlatformDirs::from_env();
            let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let report = beskar_core::doctor::examine(&dirs, &start);
            if json {
                let (document, _) = json_out::doctor("doctor", &report);
                println!("{document}");
            } else {
                render::doctor(&report);
            }
            if report.has_errors() {
                ExitCode::from(exit_codes::FAILURE)
            } else {
                ExitCode::from(exit_codes::SUCCESS)
            }
        }
        Command::Tui => unreachable!("Tui is handled in main"),
    }
}

fn dispatch_skill(action: SkillAction) -> ExitCode {
    match action {
        SkillAction::List {
            bucket,
            tag,
            profile,
            query,
            sort,
            json,
        } => {
            let ctx = Ctx {
                command: "skill list",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.list_skills(SkillFilter {
                bucket: bucket.as_deref(),
                tag: tag.as_deref(),
                profile: profile.as_deref(),
                query: query.as_deref(),
                sort: sort.map(SkillSort::from),
            }) {
                Ok(listings) => {
                    if json {
                        println!("{}", json_out::skill_list("skill list", &listings));
                    } else {
                        render::skill_list(&listings);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Show { skill, json } => {
            let ctx = Ctx {
                command: "skill show",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.show_skill(&skill) {
                Ok(detail) => {
                    if json {
                        println!("{}", json_out::skill_show("skill show", &detail));
                    } else {
                        render::skill_show(&detail);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Move {
            skill,
            bucket,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "skill move",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.skill_move(&skill, &bucket, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("skill move", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("skill move", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Rename {
            old_name,
            new_name,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "skill rename",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.skill_rename(&old_name, &new_name, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("skill rename", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("skill rename", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Remove {
            skill,
            cascade,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "skill remove",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.skill_remove(&skill, cascade, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::skill_removal("skill remove", &outcome, dry_run)
                        );
                    } else {
                        render::skill_removal(&outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Tag {
            skill,
            tags,
            remove,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "skill tag",
                json,
            };
            if tags.is_empty() && remove.is_empty() {
                return ctx.usage("name at least one tag to add or --remove <TAG>");
            }
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.skill_tag(&skill, &tags, &remove, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("skill tag", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("skill tag", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        SkillAction::Rank {
            skill,
            rank,
            clear,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "skill rank",
                json,
            };
            if rank.is_none() && !clear {
                return ctx.usage("provide a rank or --clear");
            }
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.skill_rank(&skill, rank, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("skill rank", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("skill rank", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
    }
}

fn dispatch_profile(action: ProfileAction) -> ExitCode {
    match action {
        ProfileAction::List { json } => {
            let ctx = Ctx {
                command: "profile list",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_list() {
                Ok(profiles) => {
                    if json {
                        println!("{}", json_out::profile_list("profile list", &profiles));
                    } else {
                        render::profile_list(&profiles);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Show { profile, json } => {
            let ctx = Ctx {
                command: "profile show",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_show(&profile) {
                Ok((found, attached)) => {
                    if json {
                        println!(
                            "{}",
                            json_out::profile_show("profile show", &found, &attached)
                        );
                    } else {
                        render::profile_show(&found, &attached);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Create {
            name,
            description,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile create",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_create(&name, description.as_deref(), dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile create", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile create", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Delete {
            name,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile delete",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_delete(&name, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile delete", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile delete", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Rename {
            old_name,
            new_name,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile rename",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_rename(&old_name, &new_name, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile rename", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile rename", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Add {
            profile,
            skills,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile add",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_add_skills(&profile, &skills, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile add", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile add", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Remove {
            profile,
            skills,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile remove",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_remove_skills(&profile, &skills, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile remove", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile remove", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Move {
            profile,
            skill,
            before,
            after,
            dry_run,
            json,
        } => {
            let ctx = Ctx {
                command: "profile move",
                json,
            };
            let pivot = match (before, after) {
                (Some(pivot), None) => SkillPivot::Before(pivot),
                (None, Some(pivot)) => SkillPivot::After(pivot),
                (Some(_), Some(_)) => {
                    return ctx.usage("use either --before or --after, not both");
                }
                (None, None) => {
                    return ctx.usage("profile move needs --before <skill> or --after <skill>");
                }
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_move_skill(&profile, &skill, pivot, dry_run) {
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            json_out::library_outcome("profile move", &outcome, dry_run)
                        );
                    } else {
                        render::library_outcome("profile move", &outcome, dry_run);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        ProfileAction::Validate { profile, json } => {
            let ctx = Ctx {
                command: "profile validate",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.profile_validate(profile.as_deref()) {
                Ok(reports) => {
                    let valid = reports.iter().all(|report| report.valid);
                    if json {
                        let (document, _) =
                            json_out::profile_validate("profile validate", &reports);
                        println!("{document}");
                    } else {
                        render::profile_validate(&reports);
                    }
                    if valid {
                        ExitCode::from(exit_codes::SUCCESS)
                    } else {
                        ExitCode::from(exit_codes::FAILURE)
                    }
                }
                Err(err) => ctx.fail(&err),
            }
        }
    }
}

fn dispatch_library(action: LibraryAction) -> ExitCode {
    match action {
        LibraryAction::Status { json } => {
            let ctx = Ctx {
                command: "library status",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.library_status() {
                Ok(report) => {
                    if json {
                        println!("{}", json_out::library_status("library status", &report));
                    } else {
                        render::library_status(&report);
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
        LibraryAction::Branch { action, json } => match action {
            Some(BranchAction::Create { name, json }) => {
                let ctx = Ctx {
                    command: "library branch create",
                    json,
                };
                let editor = match ctx.editor() {
                    Ok(editor) => editor,
                    Err(code) => return code,
                };
                match editor.create_branch(&name) {
                    Ok(()) => {
                        if json {
                            println!(
                                "{}",
                                json_out::envelope(
                                    "library branch create",
                                    true,
                                    serde_json::json!({"branch": name})
                                )
                            );
                        } else {
                            println!("Created and switched to branch {name:?}.");
                        }
                        ExitCode::from(exit_codes::SUCCESS)
                    }
                    Err(err) => ctx.fail(&err),
                }
            }
            None => {
                let ctx = Ctx {
                    command: "library branch",
                    json,
                };
                let editor = match ctx.editor() {
                    Ok(editor) => editor,
                    Err(code) => return code,
                };
                match editor.list_branches() {
                    Ok(branches) => {
                        if json {
                            println!("{}", json_out::branches("library branch", &branches));
                        } else {
                            render::branches(&branches);
                        }
                        ExitCode::from(exit_codes::SUCCESS)
                    }
                    Err(err) => ctx.fail(&err),
                }
            }
        },
        LibraryAction::Switch { name, json } => {
            let ctx = Ctx {
                command: "library switch",
                json,
            };
            let editor = match ctx.editor() {
                Ok(editor) => editor,
                Err(code) => return code,
            };
            match editor.switch_branch(&name) {
                Ok(()) => {
                    if json {
                        println!(
                            "{}",
                            json_out::envelope(
                                "library switch",
                                true,
                                serde_json::json!({"branch": name})
                            )
                        );
                    } else {
                        println!("Switched to branch {name:?}.");
                    }
                    ExitCode::from(exit_codes::SUCCESS)
                }
                Err(err) => ctx.fail(&err),
            }
        }
    }
}

/// Structured logging setup (spec §116). `-v/-vv` set a baseline; `RUST_LOG`
/// takes precedence. Credentials must be redacted by error producers.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbosity {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_conflicts_demand_action_not_generic_failure() {
        assert_eq!(
            exit_code(&Error::DriftConflict("modified file".into())),
            ExitCode::from(exit_codes::ACTION_REQUIRED)
        );
    }

    #[test]
    fn lock_errors_map_to_locked_code() {
        assert_eq!(
            exit_code(&Error::lock("registry busy")),
            ExitCode::from(exit_codes::LOCKED)
        );
    }

    #[test]
    fn git_auth_maps_to_git_failure() {
        assert_eq!(
            exit_code(&Error::RemoteAuth("bad token".into())),
            ExitCode::from(exit_codes::GIT_FAILURE)
        );
    }
}
