use std::env;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

pub(crate) fn acquire() -> io::Result<File> {
    let dir = match env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".local/state/manzil"),
        None => PathBuf::from("/tmp/manzil"),
    };
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
