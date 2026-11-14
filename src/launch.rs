//! Handing a photo to Windows: Show in Explorer and Open (design.md §2.4).
//!
//! File names are untrusted input, so no shell is involved: `cmd /c start` would interpret
//! `&`, `%` and `^` in a name. `explorer.exe` gets the quoted path as one raw argument instead
//! (Windows file names can't contain `"`, so the quotes can't be broken out of).

use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// Opens the photo's folder in Explorer with the file selected.
    ShowInExplorer,
    /// Opens the file with the app Windows associates with it.
    Open,
}

/// The program and its raw argument string for `kind` on `path`.
pub fn command_line(kind: Launch, path: &Path) -> (&'static str, String) {
    let quoted = format!("\"{}\"", path.display());
    match kind {
        Launch::ShowInExplorer => ("explorer.exe", format!("/select,{quoted}")),
        Launch::Open => ("explorer.exe", quoted),
    }
}

/// Starts it and returns at once; Explorer reports its own errors.
pub fn spawn(kind: Launch, path: &Path) -> io::Result<()> {
    let (program, arg) = command_line(kind, path);
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        // `raw_arg` passes the string as is; `arg` would add its own quoting around ours.
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(arg);
    }
    #[cfg(not(windows))]
    cmd.arg(arg);
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_with_spaces_commas_and_shell_characters_stay_one_argument() {
        let path = Path::new(r"Z:\Photos\2023 Rome, Italy\A&B %PATH% ^1.jpg");
        assert_eq!(
            command_line(Launch::ShowInExplorer, path),
            (
                "explorer.exe",
                r#"/select,"Z:\Photos\2023 Rome, Italy\A&B %PATH% ^1.jpg""#.to_owned()
            )
        );
        assert_eq!(
            command_line(Launch::Open, path).1,
            r#""Z:\Photos\2023 Rome, Italy\A&B %PATH% ^1.jpg""#
        );
    }
}
