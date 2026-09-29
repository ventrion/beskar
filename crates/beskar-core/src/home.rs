use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fsx::absolutize;

/// Where Beskar keeps its own files on this machine.
///
/// ```text
/// <home>/config.bsk      settings
/// <home>/registry.bsk    machine-local state
/// <home>/library/        default library location
/// ```
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
    user_home: Option<PathBuf>,
}

impl Home {
    /// Picks the Beskar home: an explicit path, else `BESKAR_HOME`, else
    /// `~/.beskar`. The caller passes what it read from the environment so this
    /// stays testable.
    pub fn resolve(
        explicit: Option<PathBuf>,
        beskar_home: Option<PathBuf>,
        user_home: Option<PathBuf>,
        cwd: &Path,
    ) -> Result<Home> {
        let root = match (explicit, beskar_home, &user_home) {
            (Some(path), _, _) | (None, Some(path), _) => absolutize(&path, cwd),
            (None, None, Some(user)) => user.join(".beskar"),
            (None, None, None) => {
                return Err(Error::invalid("cannot find a home directory")
                    .with_hint("set BESKAR_HOME or pass --home <dir>"));
            }
        };
        Ok(Home { root, user_home })
    }

    /// A home rooted at `root`, for callers that already know it.
    pub fn at(root: impl Into<PathBuf>) -> Home {
        Home { root: root.into(), user_home: None }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn user_home(&self) -> Option<&Path> {
        self.user_home.as_deref()
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.bsk")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_beats_environment_beats_default() {
        let cwd = Path::new("/work");
        let user = Some(PathBuf::from("/home/ana"));
        let home =
            Home::resolve(Some("flag".into()), Some("/env".into()), user.clone(), cwd).unwrap();
        assert_eq!(home.root(), Path::new("/work/flag"));
        let home = Home::resolve(None, Some("/env".into()), user.clone(), cwd).unwrap();
        assert_eq!(home.root(), Path::new("/env"));
        let home = Home::resolve(None, None, user, cwd).unwrap();
        assert_eq!(home.root(), Path::new("/home/ana/.beskar"));
    }

    #[test]
    fn no_home_anywhere_is_an_error_with_a_hint() {
        let error = Home::resolve(None, None, None, Path::new("/")).unwrap_err();
        assert!(error.hint().unwrap().contains("BESKAR_HOME"));
    }
}
