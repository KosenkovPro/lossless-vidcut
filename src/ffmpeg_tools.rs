//! Discovery of the external `ffmpeg` / `ffprobe` binaries.
//!
//! The application never links against FFmpeg; it shells out to the command
//! line tools. That keeps the binary small and makes the app work with any
//! FFmpeg build the user already has installed.

use std::path::{Path, PathBuf};

/// Paths to the external tools the application shells out to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// Absolute path to the `ffmpeg` executable.
    pub ffmpeg: PathBuf,
    /// Absolute path to the `ffprobe` executable.
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Look for `ffmpeg` and `ffprobe` inside `PATH`.
    pub fn detect() -> Option<Self> {
        let ffmpeg = which("ffmpeg")?;
        let ffprobe = which("ffprobe")?;
        Some(Self { ffmpeg, ffprobe })
    }

    /// Look for both binaries next to each other inside `dir`.
    pub fn from_dir(dir: &Path) -> Option<Self> {
        let ffmpeg = resolve_in_dir(dir, "ffmpeg")?;
        let ffprobe = resolve_in_dir(dir, "ffprobe")?;
        Some(Self { ffmpeg, ffprobe })
    }

    /// True when both referenced executables still exist on disk.
    pub fn valid(&self) -> bool {
        self.ffmpeg.is_file() && self.ffprobe.is_file()
    }
}

fn resolve_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    for candidate in [name.to_string(), format!("{name}.exe")] {
        let path = dir.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Search `PATH` for an executable called `name`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions = executable_extensions();
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for extension in &extensions {
            let candidate = dir.join(format!("{name}{extension}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn executable_extensions() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
    let mut out = vec![String::new()];
    out.extend(
        raw.split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| ext.to_ascii_lowercase()),
    );
    out
}

#[cfg(not(windows))]
fn executable_extensions() -> Vec<String> {
    vec![String::new()]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match path.metadata() {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_executables_inside_a_directory() {
        let dir = std::env::temp_dir().join("lossless-vidcut-tools-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fake = dir.join("ffmpeg");
        std::fs::write(&fake, b"").expect("write");
        assert_eq!(resolve_in_dir(&dir, "ffmpeg"), Some(fake));
        assert_eq!(resolve_in_dir(&dir, "definitely-not-here"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn which_finds_a_known_system_binary_or_returns_none() {
        // The result depends on the host, but the function must never panic.
        let _ = which("this-binary-does-not-exist-anywhere");
    }
}
