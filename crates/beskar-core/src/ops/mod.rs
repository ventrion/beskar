//! What a person can ask Beskar to do, one method on [`Beskar`] per
//! command, each returning a report of what it found or did.
//!
//! This is the layer a front end talks to. It owns every rule about the
//! operations themselves: which workspace a command means, what must be
//! checked before a change, what is locked while it happens and what the
//! registry records afterwards. A front end parses its input, asks its
//! questions and renders the reports; the command line does exactly that,
//! as text or as JSON, and a TUI or an editor integration could do the
//! same without repeating any of these rules.
//!
//! Changes run inside [`Beskar::transact`], so they hold the lock and read
//! the registry fresh. Read-only questions take no lock. Questions that
//! need a person (resolving a conflict, confirming a deletion) come back to
//! the front end: conflicts through a [`Resolver`], confirmations as a
//! preview the front end shows before it calls the operation that acts on
//! it.

pub mod library;
pub mod profiles;
pub mod registry;
pub mod repos;
pub mod setup;

use std::path::PathBuf;

use crate::config::ConflictPolicy;
use crate::reconcile::Resolution;
use crate::reconcile::Step;
use crate::sync::RepoPlan;
use crate::{Beskar, Error, Registry, Result, shell_quote};

/// Decides conflicts during an update: a person at a terminal, or a
/// [`ConflictPolicy`] for everyone else.
pub trait Resolver {
    /// Settle one conflict of `plan`. `None` means the workspace should be
    /// left unchanged; every conflict is settled before anything changes.
    fn resolve(&mut self, beskar: &Beskar, plan: &RepoPlan, step: &Step) -> Option<Resolution>;

    /// Whether answering may take a while (a person is asked). Such a
    /// resolver is consulted before the lock is taken, so it does not hold
    /// up other Beskar processes.
    fn asks(&self) -> bool {
        false
    }

    /// A decision about `step` no longer applies, because the skill changed
    /// after it was made; [`Resolver::resolve`] is asked again next.
    fn reconsider(&mut self, _step: &Step) {}
}

/// A policy decides every conflict the same way. `ask` has nobody to ask
/// here, so it leaves the workspace unchanged, like `abort`.
impl Resolver for ConflictPolicy {
    fn resolve(&mut self, _: &Beskar, _: &RepoPlan, _: &Step) -> Option<Resolution> {
        match self {
            ConflictPolicy::Keep => Some(Resolution::Keep),
            ConflictPolicy::Replace => Some(Resolution::Replace),
            ConflictPolicy::Ask | ConflictPolicy::Abort => None,
        }
    }
}

/// The workspace a command acts on: the registered workspace containing
/// `dir`. `named` says whether the person named `dir` (with `--repo`),
/// rather than it being the current directory, which changes the advice
/// when it is not registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRef {
    pub dir: PathBuf,
    pub named: bool,
}

impl RepoRef {
    /// The workspace around the current directory.
    pub fn here(cwd: PathBuf) -> Self {
        RepoRef {
            dir: cwd,
            named: false,
        }
    }

    /// The workspace at or around a path the person gave.
    pub fn named(dir: PathBuf) -> Self {
        RepoRef { dir, named: true }
    }
}

impl Beskar {
    /// The root of the registered workspace `at` refers to.
    pub fn find_repo(&self, registry: &Registry, at: &RepoRef) -> Result<PathBuf> {
        if at.named && !at.dir.is_dir() {
            return Err(Error::not_found(format!(
                "{} does not exist",
                self.display(&at.dir)
            )));
        }
        if let Some(entry) = registry.containing(&at.dir) {
            return Ok(entry.path.clone());
        }
        let error = Error::not_found(format!(
            "{} is not inside a registered workspace",
            self.display(&at.dir)
        ));
        Err(if at.named {
            error.hint(format!(
                "register it with `beskar repo add {}`",
                shell_quote(&self.display(&at.dir))
            ))
        } else {
            error
                .hint("register this directory with `beskar repo add .`")
                .hint("or point at a workspace with --repo <path>")
        })
    }

    /// The workspaces a status or update command acts on: every registered
    /// one, or the one `at` refers to.
    pub fn targets(&self, all: bool, at: &RepoRef) -> Result<Vec<PathBuf>> {
        let registry = self.registry()?;
        if all {
            Ok(registry.repos().map(|entry| entry.path.clone()).collect())
        } else {
            Ok(vec![self.find_repo(&registry, at)?])
        }
    }
}
