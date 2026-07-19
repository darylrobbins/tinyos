//! Path normalization shared by the kernel shell and the host tool.
//!
//! Paths are `/`-separated; a leading `/` is absolute, anything else is
//! relative to `cwd` (which must be absolute). `.` and `..` are resolved
//! lexically; `..` at the root stays at the root.

use alloc::string::String;
use alloc::vec::Vec;

use crate::layout::{FsError, MAX_NAME};

/// Resolve `path` against `cwd` into absolute components.
pub fn resolve(cwd: &str, path: &str) -> Result<Vec<String>, FsError> {
    let mut parts: Vec<String> = Vec::new();
    let absolute = path.starts_with('/');
    if !absolute {
        for c in cwd.split('/').filter(|c| !c.is_empty()) {
            parts.push(String::from(c));
        }
    }
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => {
                if name.len() > MAX_NAME {
                    return Err(FsError::NameTooLong);
                }
                parts.push(String::from(name));
            }
        }
    }
    Ok(parts)
}

/// Resolve to a canonical absolute path string ("/", "/a/b", ...).
pub fn canonical(cwd: &str, path: &str) -> Result<String, FsError> {
    let parts = resolve(cwd, path)?;
    if parts.is_empty() {
        return Ok(String::from("/"));
    }
    let mut s = String::new();
    for p in &parts {
        s.push('/');
        s.push_str(p);
    }
    Ok(s)
}
