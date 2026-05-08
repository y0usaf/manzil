use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

pub(crate) fn acquire() -> io::Result<File> {
    let home =
        env::var_os("HOME").ok_or_else(|| io::Error::new(ErrorKind::NotFound, "HOME not set"))?;
    let dir = PathBuf::from(home).join(".local/state/manzil");
    fs::create_dir_all(&dir)?;

    let path = dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;

    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(file)
}
