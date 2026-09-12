//! Events: everything that can change application state.
//!
//! Two sources feed the reducer: [`Event::Key`] from the terminal and the
//! result events produced when the runtime fulfills an
//! [`Effect`](crate::effect::Effect).

use beskar_core::editing::ProfileValidation;
use beskar_core::error::Error;

use crate::app::{ActionOutcome, MembershipView, PlannedChange, SkillView, Snapshot};
use crate::input::Key;

/// An application-level event.
#[derive(Debug)]
pub enum Event {
    /// A logical key press.
    Key(Key),
    /// [`Effect::Refresh`](crate::effect::Effect::Refresh) completed.
    Loaded(Result<Box<Snapshot>, Error>),
    /// [`Effect::LoadSkill`](crate::effect::Effect::LoadSkill) completed.
    SkillLoaded(Result<Box<SkillView>, Error>),
    /// [`Effect::Membership`](crate::effect::Effect::Membership) completed.
    MembershipLoaded(Result<Box<MembershipView>, Error>),
    /// A dry-run plan is ready (§91).
    Planned(Result<Box<PlannedChange>, Error>),
    /// A mutation finished (§89 step 5).
    Executed(Result<Box<ActionOutcome>, Error>),
    /// Profile validation finished (§76).
    Validated(Result<Vec<ProfileValidation>, Error>),
}
