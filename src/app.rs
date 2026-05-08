use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::filesystem::{activate, prune};
use crate::lock::acquire;
use crate::manifest::{read, validate, Entry};

pub(crate) fn run(new_path: &Path, old_path: Option<&Path>) -> io::Result<u32> {
    let _lock = acquire()?;

    let new = read(new_path)?;
    validate(&new, false)?;

    let old = match old_path {
        Some(path) => match read(path) {
            Ok(manifest) => {
                validate(&manifest, true)?;
                Some(manifest)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        },
        None => None,
    };

    let old_entries: HashMap<PathBuf, Entry> = old
        .as_ref()
        .map(|manifest| {
            manifest
                .files
                .iter()
                .cloned()
                .map(|entry| (entry.target.clone(), entry))
                .collect()
        })
        .unwrap_or_default();

    let mut failures = 0u32;
    let mut report = |target: &Path, result: io::Result<()>| match result {
        Ok(()) => {}
        Err(e) => {
            eprintln!("manzil: {}: {e}", target.display());
            failures += 1;
        }
    };

    if let Some(manifest) = old.as_ref() {
        let keep: HashSet<&Path> = new
            .files
            .iter()
            .map(|entry| entry.target.as_path())
            .collect();
        for entry in &manifest.files {
            if !keep.contains(entry.target.as_path()) {
                report(&entry.target, prune(entry));
            }
        }
    }

    for entry in &new.files {
        report(
            &entry.target,
            activate(entry, old_entries.get(&entry.target)),
        );
    }

    Ok(failures)
}
