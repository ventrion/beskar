use beskar_core::{Action, Beskar, Kind, Plan, Policy, RemotePlan, Result, State};
use std::io::{self, IsTerminal, Write};

pub fn interactive(json: bool) -> bool {
    !json && io::stdin().is_terminal() && io::stderr().is_terminal()
}
fn answer(question: &str) -> Result<String> {
    eprint!("{question}");
    io::stderr().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_ascii_lowercase())
}
pub fn confirm(question: &str) -> Result<bool> {
    Ok(matches!(answer(question)?.as_str(), "y" | "yes"))
}
/// Collect every decision before applying anything. Aborting preserves the entire batch.
pub fn resolve(app: &Beskar, plans: &mut [Plan]) -> Result<()> {
    for plan in plans {
        let conflicts: Vec<_> = plan
            .skills()
            .iter()
            .filter(|s| s.action() == Action::Conflict)
            .map(|s| (s.name().to_string(), s.state(), s.desired().is_some()))
            .collect();
        for (name, state, wanted) in conflicts {
            if state == State::Unmanaged {
                return Err(format!(
                    "{name}: unmanaged destination; move it aside explicitly"
                ));
            }
            eprintln!(
                "Conflict: {name}\nRepository: {}\nState: {}",
                plan.path().display(),
                state.as_str()
            );
            loop {
                let choice = answer("[k] keep local, [l] replace/remove, [d] diff, [q] abort: ")?;
                match choice.as_str() {
                    "k" | "keep" => {
                        plan.resolve(&name, Policy::Keep)?;
                        break;
                    }
                    "l" | "replace" => {
                        let question = format!(
                            "{} local changes to {name}? [y/N] ",
                            if wanted { "Replace" } else { "Delete" }
                        );
                        if confirm(&question)? {
                            plan.resolve(&name, Policy::Replace)?;
                            break;
                        }
                    }
                    "d" | "diff" => {
                        for difference in app.differences(plan.path(), &name)? {
                            eprint!("{}", difference.text);
                        }
                        eprintln!(
                            "To promote this copy, abort and run beskar skill promote {name}."
                        );
                    }
                    "q" | "quit" | "a" | "abort" | "" => {
                        return Err("update aborted; no files changed".into());
                    }
                    _ => eprintln!("Enter a complete option, such as keep or k."),
                }
            }
        }
    }
    Ok(())
}
/// The same decisions for remote sync conflicts, collected before applying.
pub fn resolve_remote(app: &Beskar, plan: &mut RemotePlan) -> Result<()> {
    let conflicts: Vec<_> = plan
        .items()
        .iter()
        .filter(|i| i.action == Action::Conflict)
        .map(|i| (i.kind, i.name.clone(), i.state, i.desired.is_some()))
        .collect();
    for (kind, name, state, wanted) in conflicts {
        if state == State::Unmanaged {
            return Err(format!(
                "{name}: unmanaged library {}; rename or remove it before syncing",
                kind.as_str()
            ));
        }
        eprintln!(
            "Conflict: {name}\nRemote: {}\nState: {}",
            plan.name,
            state.as_str()
        );
        loop {
            let choice =
                answer("[k] keep local, [l] replace with collection, [d] diff, [q] abort: ")?;
            match choice.as_str() {
                "k" | "keep" => {
                    plan.resolve(kind, &name, Policy::Keep)?;
                    break;
                }
                "l" | "replace" => {
                    let question = format!(
                        "{} local {} {name}? [y/N] ",
                        if wanted { "Replace" } else { "Delete" },
                        kind.as_str()
                    );
                    if confirm(&question)? {
                        plan.resolve(kind, &name, Policy::Replace)?;
                        break;
                    }
                }
                "d" | "diff" => {
                    if kind == Kind::Skill {
                        for difference in app.remote_diff(&plan.name, &name)? {
                            eprint!("{}", difference.text);
                        }
                    } else {
                        eprintln!("Compare profiles with beskar profile show {name}.");
                    }
                }
                "q" | "quit" | "a" | "abort" | "" => {
                    return Err("sync aborted; no files changed".into());
                }
                _ => eprintln!("Enter a complete option, such as keep or k."),
            }
        }
    }
    Ok(())
}
