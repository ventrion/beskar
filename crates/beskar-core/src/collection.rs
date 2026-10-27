//! Reading library-shaped collections out of remote clones.
//!
//! A collection mirrors the library layout: a `skills` directory of skill
//! folders, plus optional `profiles` and `metadata` directories. Collections
//! live inside Beskar-owned clones; this module only reads them.
use crate::{
    Result, format, io,
    model::{Profile, SkillMetadata},
    tree,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// A skill the collection publishes, with its metadata file when present.
#[derive(Clone, Debug)]
pub struct CollectionSkill {
    pub directory: PathBuf,
    pub fingerprint: String,
    pub metadata: Option<CollectionText>,
}

/// A profiles or metadata file the collection publishes, validated on read.
#[derive(Clone, Debug)]
pub struct CollectionText {
    pub path: PathBuf,
    pub fingerprint: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Collection {
    pub skills: BTreeMap<String, CollectionSkill>,
    pub profiles: BTreeMap<String, CollectionText>,
}

/// A tracked unit is the skill directory plus its metadata file. Both parts
/// are fixed-width fingerprints, so joining them with a dot is unambiguous.
pub fn unit(skill: &str, metadata: Option<&str>) -> String {
    match metadata {
        Some(metadata) => format!("{skill}.{metadata}"),
        None => skill.to_string(),
    }
}

impl Collection {
    /// The composite fingerprint of one skill and its metadata.
    pub fn unit_of(&self, skill: &str) -> Option<String> {
        self.skills.get(skill).map(|s| {
            unit(
                &s.fingerprint,
                s.metadata.as_ref().map(|m| m.fingerprint.as_str()),
            )
        })
    }
}

pub fn read(root: &Path) -> Result<Collection> {
    tree::safe_path(root)?;
    if !root.is_dir() {
        return Err(format!(
            "{}: remote collection is missing; run beskar remote sync",
            root.display()
        ));
    }
    let skills_root = root.join("skills");
    tree::safe_path(&skills_root)?;
    if !skills_root.is_dir() {
        return Err(format!(
            "{}: not a Beskar collection; expected a skills directory",
            root.display()
        ));
    }
    let metadata_root = root.join("metadata");
    let mut skills = BTreeMap::new();
    for child in tree::children(&skills_root)? {
        if !child.is_dir() {
            return Err(format!("{}: expected a skill directory", child.display()));
        }
        let name =
            format::path_text(Path::new(child.file_name().ok_or("missing name")?))?.to_string();
        format::name(&name)?;
        let metadata = read_text(&metadata_root.join(format!("{name}.bsk")))?;
        if let Some(metadata) = &metadata {
            let decoded = SkillMetadata::decode(&format::parse(&metadata.text)?)
                .map_err(|e| format!("{}: {e}", metadata.path.display()))?;
            for required in decoded.requires {
                // A collection is self-contained: requirements must not name
                // skills that only exist in some receiving library.
                if !tree::exists(&skills_root.join(&required))? {
                    return Err(format!(
                        "{}: skill {name} requires missing collection skill {required}",
                        metadata.path.display()
                    ));
                }
            }
        }
        if skills
            .insert(
                name.clone(),
                CollectionSkill {
                    fingerprint: tree::fingerprint(&child)?,
                    directory: child,
                    metadata,
                },
            )
            .is_some()
        {
            return Err(format!("duplicate collection skill {name}"));
        }
    }
    let profiles_root = root.join("profiles");
    let mut profiles = BTreeMap::new();
    if tree::exists(&profiles_root)? {
        for child in tree::children(&profiles_root)? {
            let raw = format::path_text(Path::new(child.file_name().ok_or("missing name")?))?;
            let name = raw
                .strip_suffix(".bsk")
                .ok_or_else(|| format!("{raw}: collection profiles must end in .bsk"))?;
            format::name(name)?;
            if let Some(profile) = read_text(&child)? {
                Profile::decode(&format::parse(&profile.text)?)
                    .map_err(|e| format!("{}: {e}", child.display()))?;
                profiles.insert(name.to_string(), profile);
            }
        }
    }
    Ok(Collection { skills, profiles })
}

fn read_text(path: &Path) -> Result<Option<CollectionText>> {
    if !tree::exists(path)? {
        return Ok(None);
    }
    Ok(Some(CollectionText {
        path: path.into(),
        fingerprint: tree::fingerprint(path)?,
        text: io(path.display(), std::fs::read_to_string(path))?,
    }))
}
