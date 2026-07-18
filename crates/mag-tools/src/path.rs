//! Worktree-relative path resolution shared by the file-system tools.
//!
//! Every built-in file-system tool receives a caller-supplied path and must keep
//! it inside the run's [`WorktreeRef`](agent_lib::agent::WorktreeRef)
//! (`docs/DESIGN.md` §7). [`safe_join`] performs a purely lexical normalization
//! that rejects absolute paths and any `..` sequence that would climb above the
//! worktree root, so a tool cannot read or write outside its sandbox even before
//! the path is touched on disk.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Reason a requested path could not be resolved inside the worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// The requested path was absolute; only worktree-relative paths are allowed.
    Absolute,
    /// The requested path escaped above the worktree root via `..`.
    Escapes,
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute => {
                formatter.write_str("path must be relative to the worktree, not absolute")
            }
            Self::Escapes => formatter.write_str("path escapes the worktree root"),
        }
    }
}

impl std::error::Error for PathError {}

/// Resolves `requested` against `worktree`, guaranteeing the result stays within
/// the worktree.
///
/// The normalization is lexical (it does not consult the file system, so it does
/// not require the path to exist and is not fooled by symlinks): `.` components
/// are dropped and each `..` pops one previously accepted component, failing
/// with [`PathError::Escapes`] if that would rise above the root. Absolute paths
/// and Windows path prefixes are rejected with [`PathError::Absolute`]. An empty
/// or `.`-only request resolves to the worktree root itself.
///
/// # Errors
///
/// Returns [`PathError`] when the requested path is absolute or would escape the
/// worktree root.
pub fn safe_join(worktree: &Path, requested: &str) -> Result<PathBuf, PathError> {
    let mut normalized = PathBuf::new();
    for component in Path::new(requested).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(PathError::Escapes);
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => return Err(PathError::Absolute),
        }
    }
    Ok(worktree.join(normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_relative_path_under_worktree() {
        let joined = safe_join(Path::new("/work"), "src/main.rs").expect("relative path resolves");
        assert_eq!(joined, PathBuf::from("/work/src/main.rs"));
    }

    #[test]
    fn empty_request_resolves_to_worktree_root() {
        let joined = safe_join(Path::new("/work"), "").expect("empty path resolves to root");
        assert_eq!(joined, PathBuf::from("/work"));
    }

    #[test]
    fn interior_parent_dir_is_normalized() {
        let joined =
            safe_join(Path::new("/work"), "src/../lib.rs").expect("interior .. normalizes");
        assert_eq!(joined, PathBuf::from("/work/lib.rs"));
    }

    #[test]
    fn escaping_parent_dir_is_rejected() {
        assert_eq!(
            safe_join(Path::new("/work"), "../secret"),
            Err(PathError::Escapes)
        );
        assert_eq!(
            safe_join(Path::new("/work"), "src/../../secret"),
            Err(PathError::Escapes)
        );
    }

    #[test]
    fn absolute_path_is_rejected() {
        assert_eq!(
            safe_join(Path::new("/work"), "/etc/passwd"),
            Err(PathError::Absolute)
        );
    }
}
