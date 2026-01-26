//! Path manipulation utilities.

use std::path::PathBuf;

/// Shorten a path by replacing home directory with ~.
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = home_dir() {
        let home_str = home.display().to_string();
        if path.starts_with(&home_str) {
            return path.replacen(&home_str, "~", 1);
        }
    }
    path.to_string()
}

/// Get the user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
