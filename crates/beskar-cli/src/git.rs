//! Git plumbing for remote collections. Beskar-core stays process-free; the
//! CLI owns every spawn. Clones under the Beskar home are Beskar's to manage,
//! so sync fetches and detaches instead of merging.
use beskar_core::Result;
use std::{
    path::Path,
    process::{Command, Output},
};

/// Where a fresh clone ended up: the reference it follows and its commit.
pub struct Checkout {
    pub reference: String,
    pub commit: String,
}

fn run(command: &mut Command) -> Result<String> {
    let output: Output = command
        .output()
        .map_err(|e| format!("cannot run git: {e}; remote commands need Git on PATH"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if message.is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            message
        };
        return Err(format!("git failed: {message}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git(clone: &Path, arguments: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(clone).args(arguments);
    run(&mut command)
}

pub fn available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Clone a collection repository. Without an explicit reference, the clone's
/// default branch becomes the reference sync follows.
pub fn clone(url: &str, reference: Option<&str>, destination: &Path) -> Result<Checkout> {
    let mut command = Command::new("git");
    command.arg("clone").arg("--quiet");
    if let Some(reference) = reference {
        command.arg("--branch").arg(reference);
    }
    command.arg(url).arg(destination);
    run(&mut command)?;
    let reference = match reference {
        Some(reference) => reference.to_string(),
        None => git(destination, &["rev-parse", "--abbrev-ref", "HEAD"])?,
    };
    Ok(Checkout {
        reference,
        commit: head(destination)?,
    })
}

/// The commit currently checked out in the clone.
pub fn head(clone: &Path) -> Result<String> {
    git(clone, &["rev-parse", "HEAD"])
}

/// Fetch the configured reference and move the clone to it. Local edits in
/// the clone make the checkout fail rather than being discarded.
pub fn fetch(clone: &Path, reference: &str) -> Result<String> {
    git(clone, &["fetch", "--quiet", "origin", reference])?;
    git(clone, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
    head(clone)
}
