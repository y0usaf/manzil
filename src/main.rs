//! manzil — tiny home file reconciler.
//!
//! Usage: manzil <new-manifest.json> [<old-manifest.json>]

mod app;
mod filesystem;
mod formats;
mod lock;
mod manifest;
mod merge;

use std::env;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (new, old) = match args.as_slice() {
        [n] => (Path::new(n), None),
        [n, o] => (Path::new(n), Some(Path::new(o))),
        _ => {
            eprintln!("usage: manzil <new-manifest.json> [<old-manifest.json>]");
            return ExitCode::from(2);
        }
    };

    match app::run(new, old) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(failures) => {
            eprintln!("manzil: {failures} entry(s) failed");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("manzil: fatal: {e}");
            ExitCode::from(1)
        }
    }
}
