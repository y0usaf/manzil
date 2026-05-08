use std::ffi::CString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::manifest::{parse_mode, Entry, EntryType};

pub(crate) fn prune(entry: &Entry) -> io::Result<()> {
    match entry.ty {
        EntryType::Symlink => prune_symlink(entry),
        EntryType::Copy => prune_copy(entry),
        EntryType::Directory => prune_directory(entry),
        EntryType::Delete | EntryType::Modify => Ok(()),
    }
}

pub(crate) fn activate(entry: &Entry, old: Option<&Entry>) -> io::Result<()> {
    match entry.ty {
        EntryType::Symlink => activate_symlink(entry, old.and_then(|e| e.source.as_deref())),
        EntryType::Copy => activate_copy(entry, old),
        EntryType::Delete => remove_path_if_present(&entry.target),
        EntryType::Directory => activate_directory(entry),
        EntryType::Modify => activate_modify(entry),
    }
}

fn prune_symlink(entry: &Entry) -> io::Result<()> {
    let source = source(entry)?;
    match fs::symlink_metadata(&entry.target) {
        Ok(m) if m.file_type().is_symlink() => {
            let current = fs::read_link(&entry.target)?;
            if current == source {
                fs::remove_file(&entry.target)?;
                eprintln!("manzil: removed {}", entry.target.display());
            } else {
                eprintln!(
                    "manzil: warning: stale entry {} is a symlink to {}, not {}, leaving alone",
                    entry.target.display(),
                    current.display(),
                    source.display()
                );
            }
            Ok(())
        }
        Ok(_) => {
            eprintln!(
                "manzil: warning: stale entry {} is not a symlink, leaving alone",
                entry.target.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn prune_copy(entry: &Entry) -> io::Result<()> {
    let source = source(entry)?;
    match fs::symlink_metadata(&entry.target) {
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && same_contents(&entry.target, source) =>
        {
            fs::remove_file(&entry.target)?;
            eprintln!("manzil: removed {}", entry.target.display());
            Ok(())
        }
        Ok(_) => {
            eprintln!(
                "manzil: warning: stale copy {} was modified, leaving alone",
                entry.target.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn prune_directory(entry: &Entry) -> io::Result<()> {
    match fs::remove_dir(&entry.target) {
        Ok(()) => {
            eprintln!("manzil: removed {}", entry.target.display());
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound || e.kind() == ErrorKind::DirectoryNotEmpty => {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn activate_symlink(entry: &Entry, old_source: Option<&Path>) -> io::Result<()> {
    let source = source(entry)?;
    ensure_parent(&entry.target)?;

    match fs::symlink_metadata(&entry.target) {
        Ok(m) if m.file_type().is_symlink() => {
            let current = fs::read_link(&entry.target)?;
            if current == source {
                return Ok(());
            }
            if entry.clobber || old_source == Some(current.as_path()) {
                return atomic_symlink(source, &entry.target);
            }

            eprintln!(
                "manzil: warning: {} is a symlink to {}, not {}, leaving alone",
                entry.target.display(),
                current.display(),
                source.display()
            );
            Ok(())
        }
        Ok(m) if entry.clobber => {
            if m.is_dir() {
                return Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "refusing to clobber directory",
                ));
            }
            atomic_symlink(source, &entry.target)?;
            eprintln!("manzil: clobbered {}", entry.target.display());
            Ok(())
        }
        Ok(_) => {
            eprintln!(
                "manzil: warning: {} exists and is not a symlink (set clobber = true to override)",
                entry.target.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => atomic_symlink(source, &entry.target),
        Err(e) => Err(e),
    }
}

fn activate_copy(entry: &Entry, old: Option<&Entry>) -> io::Result<()> {
    let source = source(entry)?;
    ensure_parent(&entry.target)?;

    match fs::symlink_metadata(&entry.target) {
        Ok(m) if m.file_type().is_symlink() => replace_copy_or_warn(entry, false),
        Ok(m) if m.is_dir() => {
            if entry.clobber {
                Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "refusing to clobber directory",
                ))
            } else {
                eprintln!(
                    "manzil: warning: {} is a directory, leaving alone",
                    entry.target.display()
                );
                Ok(())
            }
        }
        Ok(_) if same_contents(&entry.target, source) => apply_metadata(&entry.target, entry),
        Ok(_) => {
            let owned = old
                .and_then(|e| e.source.as_deref())
                .is_some_and(|old_source| same_contents(&entry.target, old_source));
            replace_copy_or_warn(entry, owned)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            atomic_copy(source, &entry.target)?;
            apply_metadata(&entry.target, entry)
        }
        Err(e) => Err(e),
    }
}

fn replace_copy_or_warn(entry: &Entry, owned: bool) -> io::Result<()> {
    if entry.clobber || owned {
        atomic_copy(source(entry)?, &entry.target)?;
        apply_metadata(&entry.target, entry)?;
        eprintln!("manzil: copied {}", entry.target.display());
    } else {
        eprintln!(
            "manzil: warning: {} exists and is not an owned copy (set clobber = true to override)",
            entry.target.display()
        );
    }
    Ok(())
}

fn activate_directory(entry: &Entry) -> io::Result<()> {
    ensure_parent(&entry.target)?;

    match fs::symlink_metadata(&entry.target) {
        Ok(m) if m.is_dir() => apply_metadata(&entry.target, entry),
        Ok(_) if entry.clobber => {
            remove_path(&entry.target)?;
            fs::create_dir(&entry.target)?;
            apply_metadata(&entry.target, entry)
        }
        Ok(_) => {
            eprintln!(
                "manzil: warning: {} exists and is not a directory (set clobber = true to override)",
                entry.target.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            fs::create_dir(&entry.target)?;
            apply_metadata(&entry.target, entry)
        }
        Err(e) => Err(e),
    }
}

fn activate_modify(entry: &Entry) -> io::Result<()> {
    match fs::symlink_metadata(&entry.target) {
        Ok(_) => apply_metadata(&entry.target, entry),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn remove_path_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => remove_path(path),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn source(entry: &Entry) -> io::Result<&Path> {
    entry
        .source
        .as_deref()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "missing source"))
}

fn apply_metadata(path: &Path, entry: &Entry) -> io::Result<()> {
    if entry.uid.is_some() || entry.gid.is_some() {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path contains NUL"))?;
        let uid = entry.uid.map(|v| v as libc::uid_t).unwrap_or(!0);
        let gid = entry.gid.map(|v| v as libc::gid_t).unwrap_or(!0);
        if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    if let Some(mode) = &entry.permissions {
        fs::set_permissions(path, fs::Permissions::from_mode(parse_mode(mode)?))?;
    }

    Ok(())
}

fn atomic_copy(source: &Path, target: &Path) -> io::Result<()> {
    let mut last_exists = None;

    for _ in 0..128 {
        let tmp = sibling_tmp(target);
        match fs::copy(source, &tmp) {
            Ok(_) => {
                if let Err(e) = fs::rename(&tmp, target) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => last_exists = Some(e),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }

    Err(last_exists.unwrap_or_else(|| {
        io::Error::new(
            ErrorKind::AlreadyExists,
            "could not allocate temporary copy",
        )
    }))
}

fn atomic_symlink(source: &Path, target: &Path) -> io::Result<()> {
    let mut last_exists = None;

    for _ in 0..128 {
        let tmp = sibling_tmp(target);
        match symlink(source, &tmp) {
            Ok(()) => {
                if let Err(e) = fs::rename(&tmp, target) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => last_exists = Some(e),
            Err(e) => return Err(e),
        }
    }

    Err(last_exists.unwrap_or_else(|| {
        io::Error::new(
            ErrorKind::AlreadyExists,
            "could not allocate temporary symlink",
        )
    }))
}

static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

fn sibling_tmp(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_owned()).unwrap_or_default();
    let id = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".manzil-tmp.{}.{}", process::id(), id));
    target.with_file_name(name)
}
