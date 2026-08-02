use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::LunaError;

pub(crate) const CODEX_BINARY_ENV: &str = "CODEX_BINARY";

pub(crate) fn resolve_codex_binary() -> Result<PathBuf, LunaError> {
    if let Some(explicit) = env::var_os(CODEX_BINARY_ENV).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir()
                .map(|directory| directory.join(&path))
                .unwrap_or(path)
        };
        if is_executable_file(&path) {
            return Ok(path);
        }
        return Err(LunaError::Dependency(format!(
            "{CODEX_BINARY_ENV} does not point to an executable file; install Codex or correct {CODEX_BINARY_ENV}, then run `luna doctor --summary`"
        )));
    }

    let path = env::var_os("PATH").ok_or_else(|| {
        LunaError::Dependency(
            "PATH is not set; install Codex and make its binary available on PATH, then run `luna doctor --summary`"
                .to_string(),
        )
    })?;
    let directories = env::split_paths(&path);
    let extensions = executable_extensions();
    resolve_executable_in("codex", directories, &extensions).ok_or_else(|| {
        LunaError::Dependency(
            "could not locate `codex` on PATH; install Codex, then run `luna doctor --summary`"
                .to_string(),
        )
    })
}

fn executable_extensions() -> Vec<OsString> {
    #[cfg(windows)]
    {
        let raw = env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
        raw.to_string_lossy()
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(OsString::from)
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![OsString::new()]
    }
}

fn resolve_executable_in(
    program: &str,
    directories: impl IntoIterator<Item = PathBuf>,
    extensions: &[OsString],
) -> Option<PathBuf> {
    let program_path = Path::new(program);
    for directory in directories {
        for extension in extensions {
            let mut name = OsString::from(program_path.as_os_str());
            if !extension.is_empty() && program_path.extension().is_none() {
                name.push(extension);
            }
            let candidate = directory.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn display_path_redacted(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("<resolved executable>")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn path_resolution_supports_spaces_and_unicode() {
        let directory = temp_dir("path with spaces-月");
        let executable = directory.join("codex");
        fs::write(&executable, b"test").expect("write executable");
        make_executable(&executable);

        let resolved = resolve_executable_in("codex", [directory.clone()], &[OsString::new()]);
        assert_eq!(resolved.as_deref(), Some(executable.as_path()));

        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn path_resolution_uses_platform_suffix_candidates() {
        let directory = temp_dir("suffix");
        let executable = directory.join("codex.EXE");
        fs::write(&executable, b"test").expect("write executable");
        make_executable(&executable);

        let resolved =
            resolve_executable_in("codex", [directory.clone()], &[OsString::from(".EXE")]);
        assert_eq!(resolved.as_deref(), Some(executable.as_path()));

        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn path_resolution_ignores_non_executable_files() {
        let directory = temp_dir("not-executable");
        let candidate = directory.join("codex");
        fs::write(&candidate, b"test").expect("write candidate");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600))
                .expect("set permissions");
        }

        let resolved = resolve_executable_in("codex", [directory.clone()], &[OsString::new()]);
        #[cfg(unix)]
        assert!(resolved.is_none());
        #[cfg(not(unix))]
        assert_eq!(resolved.as_deref(), Some(candidate.as_path()));

        fs::remove_dir_all(directory).expect("cleanup");
    }

    fn temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "luna-environment-{label}-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("set executable permissions");
        }
    }
}
