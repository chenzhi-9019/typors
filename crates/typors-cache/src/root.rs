//! Where the cache lives, and whether it is usable at all.

use std::path::{Path, PathBuf};

/// Which candidate won.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheRootKind {
    /// `<directory containing typors.exe>\cache`.
    ExeDir,
    /// `%LOCALAPPDATA%\typors\cache`, used when the executable directory is
    /// read-only (for example an all-users install under Program Files).
    LocalAppData,
    /// No candidate was writable; the session runs with in-memory caches only.
    Disabled,
}

/// The resolved cache root plus why it was chosen.
#[derive(Clone, Debug)]
pub struct CacheRoot {
    path: Option<PathBuf>,
    kind: CacheRootKind,
    reason: String,
}

/// The executable's directory, or `None` when the OS cannot report it.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// `%LOCALAPPDATA%`, or `None` when it is unset.
pub fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

impl CacheRoot {
    /// Resolve against the running executable. TYPORS_CACHE_DIR overrides the
    /// whole search, for probes and portable setups.
    pub fn resolve() -> Self {
        if let Ok(dir) = std::env::var("TYPORS_CACHE_DIR") {
            let candidate = PathBuf::from(dir);
            return match ensure_writable_dir(&candidate) {
                Ok(()) => Self {
                    path: Some(candidate.clone()),
                    kind: CacheRootKind::ExeDir,
                    reason: format!("TYPORS_CACHE_DIR={}", candidate.display()),
                },
                Err(error) => Self {
                    path: None,
                    kind: CacheRootKind::Disabled,
                    reason: format!("TYPORS_CACHE_DIR: {error}"),
                },
            };
        }
        Self::resolve_from(exe_dir(), local_app_data())
    }

    /// Resolve with injected candidates so tests never depend on the host.
    pub fn resolve_from(exe: Option<PathBuf>, local: Option<PathBuf>) -> Self {
        let mut reasons = Vec::new();
        if let Some(dir) = exe {
            let candidate = dir.join("cache");
            match ensure_writable_dir(&candidate) {
                Ok(()) => {
                    return Self {
                        path: Some(candidate),
                        kind: CacheRootKind::ExeDir,
                        reason: "cache beside typors.exe".to_string(),
                    };
                }
                Err(error) => reasons.push(format!("{}: {error}", candidate.display())),
            }
        } else {
            reasons.push("executable directory unavailable".to_string());
        }
        if let Some(dir) = local {
            let candidate = dir.join("typors").join("cache");
            match ensure_writable_dir(&candidate) {
                Ok(()) => {
                    let reason = format!("fallback to {}", candidate.display());
                    return Self {
                        path: Some(candidate),
                        kind: CacheRootKind::LocalAppData,
                        reason,
                    };
                }
                Err(error) => reasons.push(format!("{}: {error}", candidate.display())),
            }
        } else {
            reasons.push("%LOCALAPPDATA% unavailable".to_string());
        }
        Self {
            path: None,
            kind: CacheRootKind::Disabled,
            reason: reasons.join("; "),
        }
    }

    /// Root directory, or `None` when caching is disabled.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn kind(&self) -> CacheRootKind {
        self.kind
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    /// One human-readable line for `--cache-status`.
    pub fn describe(&self) -> String {
        match &self.path {
            Some(path) => format!("{} ({:?}) - {}", path.display(), self.kind, self.reason),
            None => format!("disabled - {}", self.reason),
        }
    }
}

/// Create the directory if needed and prove it is writable by writing and
/// removing a probe file. `create_dir_all` succeeding is not enough: an
/// existing read-only directory also "succeeds".
fn ensure_writable_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("create failed: {error}"))?;
    let probe = path.join(format!(".write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"typors").map_err(|error| format!("write failed: {error}"))?;
    std::fs::remove_file(&probe).map_err(|error| format!("remove failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("typors-cache-root-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn exe_directory_wins_when_writable() {
        let exe = temp("exe");
        let local = temp("local");
        let root = CacheRoot::resolve_from(Some(exe.clone()), Some(local.clone()));
        assert_eq!(root.kind(), CacheRootKind::ExeDir);
        assert_eq!(root.path(), Some(exe.join("cache").as_path()));
        assert!(exe.join("cache").is_dir());
        let _ = std::fs::remove_dir_all(&exe);
        let _ = std::fs::remove_dir_all(&local);
    }

    #[test]
    fn falls_back_when_the_exe_candidate_cannot_be_created() {
        let blocker = temp("blocker");
        // A *file* where the cache directory should be makes creation fail.
        std::fs::write(blocker.join("cache"), b"not a directory").unwrap();
        let local = temp("local2");
        let root = CacheRoot::resolve_from(Some(blocker.clone()), Some(local.clone()));
        assert_eq!(root.kind(), CacheRootKind::LocalAppData);
        assert!(root.path().unwrap().ends_with("cache"));
        let _ = std::fs::remove_dir_all(&blocker);
        let _ = std::fs::remove_dir_all(&local);
    }

    #[test]
    fn disabled_when_no_candidate_is_usable() {
        let blocker = temp("blocker2");
        std::fs::write(blocker.join("cache"), b"x").unwrap();
        let root = CacheRoot::resolve_from(Some(blocker.clone()), None);
        assert_eq!(root.kind(), CacheRootKind::Disabled);
        assert!(!root.is_enabled());
        assert!(root.describe().starts_with("disabled"));
        let _ = std::fs::remove_dir_all(&blocker);
    }
}
