//! Asking a person how to settle a conflict, the command line's
//! [`Resolver`].

use beskar_core::diff;
use beskar_core::ops::Resolver;
use beskar_core::reconcile::{Action, Conflict, Resolution, Step};
use beskar_core::sync::RepoPlan;
use beskar_core::{Beskar, ConflictPolicy};

use crate::app::App;
use crate::cmd::repo::render_diff;

/// Diff lines shown at the prompt before pointing at `beskar repo diff`.
const DIFF_LINES: usize = 120;

/// Settles conflicts by `policy`, asking at the terminal when the policy is
/// `ask` and there is someone to ask.
pub struct Prompt<'a> {
    app: &'a App,
    policy: ConflictPolicy,
}

impl<'a> Prompt<'a> {
    pub fn new(app: &'a App, policy: ConflictPolicy) -> Self {
        Prompt { app, policy }
    }
}

impl Resolver for Prompt<'_> {
    fn asks(&self) -> bool {
        self.policy == ConflictPolicy::Ask && self.app.env.interactive
    }

    fn reconsider(&mut self, step: &Step) {
        self.app.out.err_line(format!(
            "{}: {} changed while you were deciding, so here it is again.",
            self.app.err_style().bold("note"),
            step.skill
        ));
    }

    fn resolve(&mut self, beskar: &Beskar, plan: &RepoPlan, step: &Step) -> Option<Resolution> {
        if self.asks() {
            ask(self.app, beskar, plan, step)
        } else {
            self.policy.resolve(beskar, plan, step)
        }
    }
}

fn ask(app: &App, beskar: &Beskar, plan: &RepoPlan, step: &Step) -> Option<Resolution> {
    let style = app.err_style();
    let (explanation, keep, replace, promote) = match step.action {
        Action::Conflict(Conflict::Orphaned) => (
            "The workspace copy has local changes, and no enabled profile includes it any more.",
            "keep it here, no longer managed",
            "delete it",
            "save it to the library, then delete it here",
        ),
        Action::Conflict(Conflict::Untracked) => (
            "This directory differs from the library version, and Beskar does not manage it.",
            "keep it",
            "replace it with the library version",
            "make it the library version, replacing the one there",
        ),
        _ => (
            "The workspace copy has local changes, and the library has a newer version.",
            "keep local",
            "replace with library",
            "promote to library, overwriting its newer version",
        ),
    };
    app.out.err_line("");
    app.out.err_line(format!(
        "{} {} in {}",
        style.bold_red("Conflict:"),
        style.bold(step.skill.as_str()),
        app.display(&plan.repo)
    ));
    app.out.err_line(explanation);
    app.out.err_line("");
    for (key, text) in [
        ("k", keep),
        ("l", replace),
        ("p", promote),
        ("d", "show diff"),
        ("a", "abort: change nothing here"),
    ] {
        app.out.err_line(format!("  [{}] {text}", style.bold(key)));
    }
    loop {
        match app.choose("Choice: ", &['k', 'l', 'p', 'd', 'a'])? {
            'k' => return Some(Resolution::Keep),
            'l' => {
                let question = if step.action == Action::Conflict(Conflict::Orphaned) {
                    format!("Delete {} and its local changes?", step.skill)
                } else {
                    format!("Discard the local changes to {}?", step.skill)
                };
                if app.confirm(&question, false)? {
                    return Some(Resolution::Replace);
                }
            }
            'p' if step.action == Action::Conflict(Conflict::Orphaned)
                && !beskar.library.contains(&step.skill) =>
            {
                return Some(Resolution::Promote);
            }
            'p' => {
                let question = format!(
                    "Overwrite the library's version of {} with this copy?",
                    step.skill
                );
                if app.confirm(&question, false)? {
                    return Some(Resolution::Promote);
                }
            }
            'd' => show_diff(app, beskar, plan, step),
            _ => return None,
        }
    }
}

fn show_diff(app: &App, beskar: &Beskar, plan: &RepoPlan, step: &Step) {
    let library = beskar.library.skill_source(&step.skill);
    let workspace = beskar.workspace(&plan.repo).skill_path(&step.skill);
    match diff::compare(&library, &workspace, beskar.ignore()) {
        Ok(diffs) => {
            let lines = render_diff(&step.skill, &diffs, app.err_style());
            let total = lines.len();
            for line in lines.into_iter().take(DIFF_LINES) {
                app.out.err_line(line);
            }
            if total > DIFF_LINES {
                app.out.err_line(format!(
                    "… {} more lines; `beskar repo diff {}` shows them all",
                    total - DIFF_LINES,
                    step.skill
                ));
            }
        }
        Err(err) => app.out.err_line(format!("cannot compare: {err}")),
    }
}
