//! Small platform specific helpers.
//!
//! The application ships as a GUI program. On Windows that means it is
//! linked with the `windows` subsystem, which has no console attached. Every
//! child process (`ffmpeg`, `ffprobe`, PowerShell) would nevertheless open a
//! console window of its own, so the flag that suppresses it is applied to
//! every process the application starts.

/// The `CREATE_NO_WINDOW` flag of the Windows process creation API.
///
/// The value is documented in the Windows SDK and is stable.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Hide the console window of a child process on Windows.
///
/// On every other platform this does nothing, because no console is attached
/// to a graphical process in the first place.
#[cfg(windows)]
pub fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Hide the console window of a child process on Windows.
#[cfg(not(windows))]
pub fn hide_console(_command: &mut std::process::Command) {}

/// Hide the console window of an asynchronous child process on Windows.
#[cfg(windows)]
pub fn hide_console_tokio(command: &mut tokio::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Hide the console window of an asynchronous child process on Windows.
#[cfg(not(windows))]
pub fn hide_console_tokio(_command: &mut tokio::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_flag_matches_the_sdk() {
        #[cfg(windows)]
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
