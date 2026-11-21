//! Desktop shortcut management.
//!
//! The application can put a shortcut on the desktop of the user that starts
//! the program. Every platform needs a different trick for that: Windows uses
//! the `WScript.Shell` COM object, Linux writes a freedesktop entry and macOS
//! asks Finder to create an alias.
//!
//! The platform specific string building lives in pure functions that take the
//! paths as arguments, so that it can be unit tested on every platform.

use std::path::PathBuf;

use anyhow::Result;

/// The display name of the shortcut, without any file extension.
pub const NAME: &str = "Lossless VidCut";

/// Create a shortcut on the desktop that starts the running application.
/// Returns the path of the created shortcut.
pub fn create() -> Result<PathBuf> {
    platform::create()
}

/// Remove the desktop shortcut again. Succeeds even when there is nothing to remove.
pub fn remove() -> Result<()> {
    platform::remove()
}

/// `true` when a desktop shortcut created by this module exists.
pub fn exists() -> bool {
    platform::exists()
}

/// Windows implementation: a `.lnk` file inside the desktop directory.
#[cfg(windows)]
mod platform {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;

    use anyhow::{Context, Result};
    use std::os::windows::process::CommandExt;

    /// `CREATE_NO_WINDOW`: start PowerShell without a flashing console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn create() -> Result<PathBuf> {
        let target = std::env::current_exe()
            .context("could not determine the path of the running executable")?;
        run_powershell(&super::powershell_script(&target))?;
        Ok(super::link_path())
    }

    pub fn remove() -> Result<()> {
        let link = super::link_path();
        match std::fs::remove_file(&link) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("could not remove {}", link.display()))
            }
        }
    }

    pub fn exists() -> bool {
        super::link_path().is_file()
    }

    /// The desktop directory of the current user.
    ///
    /// Windows may move the desktop to another drive, for example into
    /// OneDrive, so the directory is asked for instead of being guessed. The
    /// answer is cached, because the lookup needs a helper process.
    pub(super) fn desktop_dir() -> PathBuf {
        static DESKTOP: OnceLock<PathBuf> = OnceLock::new();
        DESKTOP
            .get_or_init(|| {
                let script = "[Environment]::GetFolderPath('Desktop')";
                let answer = Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", script])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                    .filter(|text| !text.is_empty());
                match answer {
                    Some(path) => PathBuf::from(path),
                    None => match dirs::home_dir() {
                        Some(home) => home.join("Desktop"),
                        None => PathBuf::from("Desktop"),
                    },
                }
            })
            .clone()
    }

    /// Run a PowerShell script and turn a failing exit status into an error.
    fn run_powershell(script: &str) -> Result<()> {
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .context("could not run PowerShell")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut message = format!("PowerShell exited with {}", output.status);
            let details = stderr.trim();
            if !details.is_empty() {
                message.push_str(": ");
                message.push_str(details);
            } else {
                let printed = stdout.trim();
                if !printed.is_empty() {
                    message.push_str(": ");
                    message.push_str(printed);
                }
            }
            anyhow::bail!(message);
        }
        Ok(())
    }
}

/// Linux implementation: a freedesktop entry plus a copy on the desktop.
#[cfg(target_os = "linux")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result};

    /// File name of the freedesktop entry.
    const ENTRY_FILE: &str = "lossless-vidcut.desktop";

    pub fn create() -> Result<PathBuf> {
        let exe = std::env::current_exe()
            .context("could not determine the path of the running executable")?;
        let entry = super::desktop_entry(&exe);

        let applications = applications_dir()?;
        std::fs::create_dir_all(&applications)
            .with_context(|| format!("could not create {}", applications.display()))?;
        write_entry(&applications.join(ENTRY_FILE), &entry)?;

        let desktop = desktop_dir()?;
        std::fs::create_dir_all(&desktop)
            .with_context(|| format!("could not create {}", desktop.display()))?;
        let shortcut = desktop.join(ENTRY_FILE);
        write_entry(&shortcut, &entry)?;
        Ok(shortcut)
    }

    pub fn remove() -> Result<()> {
        if let Ok(applications) = applications_dir() {
            remove_file(&applications.join(ENTRY_FILE))?;
        }
        if let Ok(desktop) = desktop_dir() {
            remove_file(&desktop.join(ENTRY_FILE))?;
        }
        Ok(())
    }

    pub fn exists() -> bool {
        let installed = match applications_dir() {
            Ok(applications) => applications.join(ENTRY_FILE).is_file(),
            Err(_) => false,
        };
        let shortcut = match desktop_dir() {
            Ok(desktop) => desktop.join(ENTRY_FILE).is_file(),
            Err(_) => false,
        };
        installed || shortcut
    }

    /// The directory that holds the installed applications.
    fn applications_dir() -> Result<PathBuf> {
        let data = dirs::data_dir().context("no data directory available")?;
        Ok(data.join("applications"))
    }

    /// The desktop directory, as reported by `xdg-user-dir`.
    fn desktop_dir() -> Result<PathBuf> {
        if let Ok(output) = Command::new("xdg-user-dir").arg("DESKTOP").output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Ok(PathBuf::from(trimmed));
                }
            }
        }
        let home = dirs::home_dir().context("could not determine the home directory")?;
        Ok(home.join("Desktop"))
    }

    /// Write the entry and make it executable.
    fn write_entry(path: &Path, content: &str) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, content)
            .with_context(|| format!("could not write {}", path.display()))?;
        let mut permissions = std::fs::metadata(path)
            .with_context(|| format!("could not read the permissions of {}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("could not set the permissions of {}", path.display()))
    }

    /// Remove a file and ignore a missing one.
    fn remove_file(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("could not remove {}", path.display()))
            }
        }
    }
}

/// macOS implementation: a Finder alias on the desktop.
#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result};

    pub fn create() -> Result<PathBuf> {
        let exe = std::env::current_exe()
            .context("could not determine the path of the running executable")?;
        let desktop = desktop_dir()?;
        let script = super::finder_alias_script(&exe, &desktop);

        // `osascript` is started without a terminal, the application itself
        // never shows a console window.
        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .context("could not run osascript")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("osascript exited with {}: {}", output.status, stderr.trim());
        }

        let printed = String::from_utf8_lossy(&output.stdout);
        let printed = printed.trim();
        if !printed.is_empty() {
            return Ok(PathBuf::from(printed));
        }
        match find_alias(Some(&exe), &desktop) {
            Some(alias) => Ok(alias),
            None => Err(anyhow::anyhow!(
                "the alias was created but could not be found on the desktop"
            )),
        }
    }

    pub fn remove() -> Result<()> {
        let desktop = desktop_dir()?;
        let exe = std::env::current_exe().ok();
        let alias = match find_alias(exe.as_deref(), &desktop) {
            Some(alias) => alias,
            None => return Ok(()),
        };
        match std::fs::remove_file(&alias) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("could not remove {}", alias.display()))
            }
        }
    }

    pub fn exists() -> bool {
        let desktop = match desktop_dir() {
            Ok(desktop) => desktop,
            Err(_) => return false,
        };
        let exe = std::env::current_exe().ok();
        find_alias(exe.as_deref(), &desktop).is_some()
    }

    /// The desktop directory of the current user.
    fn desktop_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine the home directory")?;
        Ok(home.join("Desktop"))
    }

    /// Look for the alias that Finder created for the executable.
    fn find_alias(exe: Option<&Path>, desktop: &Path) -> Option<PathBuf> {
        let prefix = match exe.and_then(|path| path.file_name()) {
            Some(name) => name.to_string_lossy().into_owned(),
            None => super::NAME.to_string(),
        };
        let entries = std::fs::read_dir(desktop).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name.ends_with(" alias") {
                return Some(entry.path());
            }
        }
        None
    }
}

/// Fallback for platforms without a desktop shortcut implementation.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use std::path::PathBuf;

    use anyhow::Result;

    pub fn create() -> Result<PathBuf> {
        anyhow::bail!("desktop shortcuts are not supported on this platform")
    }

    pub fn remove() -> Result<()> {
        Ok(())
    }

    pub fn exists() -> bool {
        false
    }
}

/// The path of the `.lnk` file on the desktop.
#[cfg(windows)]
fn link_path() -> std::path::PathBuf {
    platform::desktop_dir().join(format!("{NAME}.lnk"))
}

/// Quote a path for use inside a PowerShell single quoted string.
///
/// Single quotes are the only escape that PowerShell knows inside a single
/// quoted string, so they are doubled.
#[cfg(windows)]
fn quote_powershell(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', "''"))
}

/// Build the PowerShell script that creates the desktop shortcut.
///
/// The script is a pure function of the given target path, so that it can be
/// tested without touching the file system.
#[cfg(windows)]
fn powershell_script(target: &std::path::Path) -> String {
    let exe = quote_powershell(target);
    let working_dir = quote_powershell(&exe_directory(target));
    let icon = quote_powershell(std::path::Path::new(&format!("{},0", target.display())));
    let description = quote_powershell(std::path::Path::new(NAME));
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $shell = New-Object -ComObject WScript.Shell\n\
         $shortcut = $shell.CreateShortcut({link})\n\
         $shortcut.TargetPath = {exe}\n\
         $shortcut.WorkingDirectory = {working_dir}\n\
         $shortcut.IconLocation = {icon}\n\
         $shortcut.Description = {description}\n\
         $shortcut.Save()\n",
        link = quote_powershell(&link_path()),
        exe = exe,
        working_dir = working_dir,
        icon = icon,
        description = description,
    )
}

/// Build the contents of a freedesktop entry for the given executable.
///
/// The helper is only used by the Linux implementation, but it is compiled on
/// macOS as well so that it can be tested there.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn desktop_entry(exec: &std::path::Path) -> String {
    let directory = exe_directory(exec);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Cut and join videos without re-encoding\n\
         Exec={exec}\n\
         Path={directory}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=AudioVideo;Video;\n",
        name = NAME,
        exec = desktop_exec(exec),
        directory = directory.to_string_lossy(),
        icon = exec.to_string_lossy(),
    )
}

/// Quote the value of the `Exec` key of a desktop entry.
///
/// Backslashes, double quotes and single quotes are escaped with a backslash.
/// A value that contains whitespace is wrapped in double quotes.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn desktop_exec(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\'', "\\'");
    if raw.chars().any(char::is_whitespace) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// The directory that contains the given executable.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn exe_directory(path: &std::path::Path) -> std::path::PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

/// Build the AppleScript that asks Finder for an alias of the executable.
#[cfg(target_os = "macos")]
fn finder_alias_script(exe: &std::path::Path, desktop: &std::path::Path) -> String {
    format!(
        "tell application \"Finder\"\n\
         \tset newAlias to make alias file to POSIX file \"{exe}\" at POSIX file \"{desktop}\"\n\
         \ttry\n\
         \t\treturn POSIX path of (newAlias as alias)\n\
         \ton error\n\
         \t\treturn \"\"\n\
         \tend try\n\
         end tell",
        exe = quote_applescript(exe),
        desktop = quote_applescript(desktop),
    )
}

/// Quote a path for use inside an AppleScript string literal.
#[cfg(target_os = "macos")]
fn quote_applescript(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::NAME;

    #[test]
    fn name_is_exact() {
        assert_eq!(NAME, "Lossless VidCut");
    }

    #[cfg(windows)]
    mod windows {
        use std::path::Path;

        use super::super::{powershell_script, quote_powershell};

        #[test]
        fn powershell_script_contains_the_target() {
            let script = powershell_script(Path::new(r"C:\Program Files\app.exe"));
            assert!(script.contains(r"TargetPath = 'C:\Program Files\app.exe'"));
            assert!(script.contains(r"WorkingDirectory = 'C:\Program Files'"));
            assert!(script.contains(r"IconLocation = 'C:\Program Files\app.exe,0'"));
            assert!(script.contains("Description = 'Lossless VidCut'"));
            assert!(script.contains("CreateShortcut"));
            assert!(script.contains("Save()"));
        }

        #[test]
        fn powershell_script_doubles_single_quotes() {
            let script = powershell_script(Path::new(r"C:\it's here\app.exe"));
            assert!(script.contains(r"'C:\it''s here\app.exe'"));
            assert!(!script.contains(r"'C:\it's here\app.exe'"));
        }

        #[test]
        fn quote_powershell_wraps_and_escapes() {
            assert_eq!(quote_powershell(Path::new(r"C:\app.exe")), r"'C:\app.exe'");
            assert_eq!(
                quote_powershell(Path::new(r"C:\it's here\app.exe")),
                r"'C:\it''s here\app.exe'"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod unix {
        use std::path::Path;

        use super::super::desktop_entry;

        #[test]
        fn desktop_entry_contains_the_required_keys() {
            let entry = desktop_entry(Path::new("/opt/lossless-vidcut/app"));
            assert!(entry.contains("Type=Application"));
            assert!(entry.contains("Name=Lossless VidCut"));
            assert!(entry.contains("Exec=/opt/lossless-vidcut/app"));
            assert!(entry.contains("Path=/opt/lossless-vidcut"));
            assert!(entry.contains("Icon=/opt/lossless-vidcut/app"));
            assert!(entry.contains("Terminal=false"));
            assert!(entry.contains("Categories=AudioVideo;Video;"));
        }

        #[test]
        fn desktop_entry_escapes_single_quotes() {
            let entry = desktop_entry(Path::new("/home/o'brien/app"));
            let mut exec = "";
            for line in entry.lines() {
                if line.starts_with("Exec=") {
                    exec = line;
                    break;
                }
            }
            assert_eq!(exec, r"Exec=/home/o\'brien/app");
        }

        #[test]
        fn desktop_entry_quotes_exec_paths_with_spaces() {
            let entry = desktop_entry(Path::new("/opt/my app/run"));
            assert!(entry.contains(r#"Exec="/opt/my app/run""#));
        }
    }
}
