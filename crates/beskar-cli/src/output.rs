use crate::json::Json;
use beskar_core::{Action, Plan, Usage};
use std::collections::BTreeSet;

pub struct Report {
    pub text: String,
    pub data: Json,
    pub error: Option<String>,
}
impl Report {
    pub fn new(text: impl Into<String>, data: Json) -> Self {
        Self {
            text: text.into(),
            data,
            error: None,
        }
    }
    pub fn message(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(format!("{text}\n"), Json::obj([("message", text.into())]))
    }
    pub fn fail(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}
pub fn usage(items: &[Usage]) -> Json {
    Json::arr(items.iter().map(|item| {
        Json::obj([
            ("path", Json::path(&item.path)),
            ("installed", item.installed.into()),
            ("profiles", Json::strings(&item.profiles)),
            ("required_by", Json::strings(&item.required_by)),
        ])
    }))
}
pub fn plans(plans: &[Plan]) -> Json {
    Json::arr(plans.iter().map(|plan| {
        Json::obj([
            ("path", Json::path(plan.path())),
            ("unmanaged", Json::strings(plan.unmanaged())),
            (
                "skills",
                Json::arr(plan.skills().iter().map(|skill| {
                    Json::obj([
                        ("name", skill.name().into()),
                        ("action", skill.action().as_str().into()),
                        ("state", skill.state().as_str().into()),
                        ("reason", skill.reason().into()),
                        ("baseline", skill.baseline().into()),
                        ("current", skill.current().into()),
                        ("desired", skill.desired().into()),
                        ("profiles", Json::strings(skill.profiles())),
                        ("required_by", Json::strings(skill.required_by())),
                    ])
                })),
            ),
        ])
    }))
}
pub fn plan_text(plan: &Plan, unchanged: bool) -> String {
    let mut text = format!("Repository: {}\n", plan.path().display());
    for skill in plan.skills() {
        if !unchanged && skill.action() == Action::Unchanged {
            continue;
        }
        let symbol = match skill.action() {
            Action::Add => "+",
            Action::Update => "~",
            Action::Remove => "-",
            Action::Unchanged => "=",
            Action::Keep => "k",
            Action::Conflict => "!",
        };
        text.push_str(&format!(
            "  {symbol} {}  {} [{}]{}\n",
            skill.name(),
            skill.reason(),
            skill.state().as_str(),
            reasons(skill.profiles(), skill.required_by())
        ));
    }
    for name in plan.unmanaged() {
        text.push_str(&format!("  ? {name}  unmanaged, preserved\n"));
    }
    if plan.pending() == 0 {
        text.push_str("  Up to date.\n");
    }
    text
}
pub fn join<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    names
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
/// Why a skill is wanted. A skill wanted for no reason keeps an empty profile list.
pub fn reasons(profiles: &BTreeSet<String>, dependents: &BTreeSet<String>) -> String {
    if profiles.is_empty() && !dependents.is_empty() {
        required_by(dependents)
    } else {
        format!(" [profiles: {}]{}", join(profiles), required_by(dependents))
    }
}
/// A suffix naming the skills that require this one, or nothing.
pub fn required_by<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    let names = join(names);
    if names.is_empty() {
        names
    } else {
        format!(" [required by: {names}]")
    }
}
