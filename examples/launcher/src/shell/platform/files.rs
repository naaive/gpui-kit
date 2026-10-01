//! Moving files to the Trash and opening them with a chosen application.

use std::{path::PathBuf, process::Command};

use anyhow::{Context as _, Result, bail};

/// Moves `paths` to the Trash (the Recycle Bin on Windows), where the user
/// can put them back. Blocking.
pub fn trash(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    for path in paths {
        if std::fs::symlink_metadata(path).is_err() {
            bail!("{} does not exist", path.display());
        }
    }
    trash_existing(paths)
}

#[cfg(target_os = "windows")]
fn trash_existing(paths: &[PathBuf]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::{
        Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            SHFILEOPSTRUCTW, SHFileOperationW,
        },
        core::PCWSTR,
    };

    // A list of absolute paths, each ended by a NUL, the list by another.
    let mut from: Vec<u16> = Vec::new();
    for path in paths {
        let path = std::path::absolute(path)?;
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status != 0 || operation.fAnyOperationsAborted.as_bool() {
        bail!("the Recycle Bin refused the files (error {status:#x})");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn trash_existing(paths: &[PathBuf]) -> Result<()> {
    let files = paths
        .iter()
        .map(|path| {
            let path = std::path::absolute(path)?;
            Ok(format!(
                "POSIX file \"{}\"",
                path.display()
                    .to_string()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    run(Command::new("osascript").args([
        "-e",
        &format!(
            "tell application \"Finder\" to delete {{{}}}",
            files.join(", ")
        ),
    ]))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn trash_existing(paths: &[PathBuf]) -> Result<()> {
    run(Command::new("gio").arg("trash").args(paths))
}

/// Opens `target`, a file, a folder or a URL, with `application`: a path to
/// a program, or a name the system finds it by (`notepad`, `Safari`).
pub fn open_with(target: &str, application: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg("-a").arg(application).arg(target);
        command
    } else {
        let mut command = Command::new(application);
        command.arg(target);
        command
    };
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if std::path::Path::new(application)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }
    command
        .spawn()
        .with_context(|| format!("cannot start {application}"))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn run(command: &mut Command) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("cannot run {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trash_refuses_a_missing_file() {
        let error = trash(&[PathBuf::from("this/file/does/not/exist")]).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert!(trash(&[]).is_ok());
    }

    /// Moves a file of this test's own to the Recycle Bin.
    #[test]
    #[ignore = "puts a file in the user's Trash"]
    fn test_trash_moves_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("launcher-trash-test.txt");
        std::fs::write(&file, "x").unwrap();
        trash(std::slice::from_ref(&file)).unwrap();
        assert!(!file.exists());
    }
}
