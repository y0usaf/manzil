use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::formats::Format;
use crate::merge::ArrayStrategy;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    #[serde(default)]
    version: Option<u32>,
    pub(crate) files: Vec<Entry>,
}

#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EntryType {
    #[default]
    Symlink,
    Copy,
    Delete,
    Directory,
    Modify,
    Merge,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) target: PathBuf,
    #[serde(rename = "type", default)]
    pub(crate) ty: EntryType,
    #[serde(default)]
    pub(crate) source: Option<PathBuf>,
    #[serde(default)]
    pub(crate) clobber: bool,
    #[serde(default)]
    pub(crate) permissions: Option<String>,
    #[serde(default)]
    pub(crate) uid: Option<u32>,
    #[serde(default)]
    pub(crate) gid: Option<u32>,
    /// Format of the existing file a `merge` entry patches into.
    #[serde(default)]
    pub(crate) format: Option<Format>,
    /// Default array strategy for `merge` entries.
    #[serde(default, rename = "arrayDefault")]
    pub(crate) array_default: ArrayStrategy,
    /// Per-path (dot-separated) array strategy overrides for `merge` entries.
    #[serde(default)]
    pub(crate) arrays: HashMap<String, ArrayStrategy>,
}

pub(crate) fn read(path: &Path) -> io::Result<Manifest> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

pub(crate) fn validate(manifest: &Manifest, allow_legacy_version: bool) -> io::Result<()> {
    match manifest.version {
        Some(1..=3) => {}
        Some(version) => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("unsupported manifest version: {version}"),
            ));
        }
        None if allow_legacy_version => {}
        None => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "missing manifest version",
            ))
        }
    }

    let mut seen = HashSet::new();
    for entry in &manifest.files {
        validate_path("target", &entry.target)?;
        if let Some(source) = &entry.source {
            validate_path("source", source)?;
        }

        match entry.ty {
            EntryType::Symlink | EntryType::Copy | EntryType::Merge if entry.source.is_none() => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "source is required for symlink/copy/merge entries",
                ));
            }
            EntryType::Delete | EntryType::Directory | EntryType::Modify
                if entry.source.is_some() =>
            {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "source is only valid for symlink/copy/merge entries",
                ));
            }
            _ => {}
        }

        if entry.ty == EntryType::Merge && entry.format.is_none() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "format is required for merge entries",
            ));
        }
        if entry.ty != EntryType::Merge && (entry.format.is_some() || !entry.arrays.is_empty()) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "format/arrays are only valid for merge entries",
            ));
        }

        if has_metadata(entry)
            && !matches!(
                entry.ty,
                EntryType::Copy | EntryType::Directory | EntryType::Modify | EntryType::Merge
            )
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "metadata is only valid for copy/directory/modify/merge entries",
            ));
        }

        if let Some(mode) = &entry.permissions {
            parse_mode(mode)?;
        }

        if !seen.insert(entry.target.clone()) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("duplicate target in manifest: {}", entry.target.display()),
            ));
        }
    }

    Ok(())
}

pub(crate) fn parse_mode(mode: &str) -> io::Result<u32> {
    if !(mode.len() == 3 || mode.len() == 4) || !mode.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid permissions: {mode}"),
        ));
    }
    u32::from_str_radix(mode, 8).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

fn validate_path(kind: &str, path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{kind} path is empty"),
        ));
    }

    if !path.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{kind} path is not absolute: {}", path.display()),
        ));
    }

    if path == Path::new("/") {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("refusing dangerous {kind} path: {}", path.display()),
        ));
    }

    if path
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{kind} path contains '.' or '..': {}", path.display()),
        ));
    }

    Ok(())
}

fn has_metadata(entry: &Entry) -> bool {
    entry.permissions.is_some() || entry.uid.is_some() || entry.gid.is_some()
}
