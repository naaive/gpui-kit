//! Starting the launcher when the user logs in, hidden until it is summoned.
//!
//! Each platform keeps the list in its own place: the `Run` key of the
//! user's registry on Windows, a LaunchAgent on macOS and an XDG autostart
//! entry on Linux. The entry runs `launcher start --background`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// What the entry is called where the user can see it: the value in the
/// `Run` key, the autostart entry's name.
const NAME: &str = "GPUI Kit Launcher";

/// The LaunchAgent's label, which is also its file name.
const LABEL: &str = "com.gpui-kit.launcher";

/// The arguments that start the launcher without showing its window.
const ARGUMENTS: [&str; 2] = ["start", "--background"];

/// Adds the launcher to what starts at login, pointing at this executable,
/// or removes it.
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    let executable = std::env::current_exe().context("cannot find the launcher's executable")?;
    #[cfg(target_os = "windows")]
    {
        registry::set(enabled.then(|| run_command(&executable)).as_deref())
    }
    #[cfg(target_os = "macos")]
    {
        let contents = enabled.then(|| launch_agent(&executable)).transpose()?;
        write_or_remove(&entry_path()?, contents.as_deref())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let contents = enabled.then(|| autostart_entry(&executable));
        write_or_remove(&entry_path()?, contents.as_deref())
    }
}

/// Whether the launcher is among what starts at login.
pub fn is_launching_at_login() -> bool {
    #[cfg(target_os = "windows")]
    {
        registry::is_set()
    }
    #[cfg(not(target_os = "windows"))]
    {
        entry_path().is_ok_and(|path| path.is_file())
    }
}

/// The LaunchAgent or autostart file.
#[cfg(not(target_os = "windows"))]
fn entry_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let path = dirs::home_dir().map(|home| launch_agent_path(&home));
    #[cfg(not(target_os = "macos"))]
    let path = dirs::config_dir().map(|config| autostart_path(&config));
    path.context("this system has no home directory")
}

#[cfg(not(target_os = "windows"))]
fn write_or_remove(path: &Path, contents: Option<&str>) -> Result<()> {
    match contents {
        Some(contents) => {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)
                    .with_context(|| format!("cannot create {}", directory.display()))?;
            }
            std::fs::write(path, contents)
                .with_context(|| format!("cannot write {}", path.display()))
        }
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("cannot remove {}", path.display()))
            }
            _ => Ok(()),
        },
    }
}

/// The command line in the `Run` key, the executable quoted for its spaces.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn run_command(executable: &Path) -> String {
    format!("\"{}\" {}", executable.display(), ARGUMENTS.join(" "))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The LaunchAgent property list, which runs the launcher once at login.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent(executable: &Path) -> Result<String> {
    let arguments = std::iter::once(executable.to_string_lossy().into_owned())
        .chain(ARGUMENTS.map(str::to_owned))
        .map(plist::Value::String)
        .collect();
    let agent = plist::Dictionary::from_iter([
        ("Label", plist::Value::String(LABEL.into())),
        ("ProgramArguments", plist::Value::Array(arguments)),
        ("RunAtLoad", plist::Value::Boolean(true)),
    ]);
    let mut text = Vec::new();
    plist::Value::Dictionary(agent).to_writer_xml(&mut text)?;
    Ok(String::from_utf8(text)?)
}

#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn autostart_path(config: &Path) -> PathBuf {
    config.join("autostart").join("gpui-kit-launcher.desktop")
}

/// The XDG autostart entry. `Exec` quotes the executable as the Desktop
/// Entry specification asks, then escapes the backslashes once more because
/// the value is itself a string.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn autostart_entry(executable: &Path) -> String {
    let mut quoted = String::from("\"");
    for character in executable.to_string_lossy().chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    let exec = format!("{quoted} {}", ARGUMENTS.join(" ")).replace('\\', "\\\\");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={NAME}\n\
         Exec={exec}\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(target_os = "windows")]
mod registry {
    use windows::{
        Win32::{
            Foundation::ERROR_FILE_NOT_FOUND,
            System::Registry::{
                HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
                RegSetKeyValueW,
            },
        },
        core::{HSTRING, PCWSTR, w},
    };

    const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

    /// Writes `command` as the launcher's value in the `Run` key, or removes
    /// the value.
    pub fn set(command: Option<&str>) -> anyhow::Result<()> {
        let name = HSTRING::from(super::NAME);
        let status = match command {
            Some(command) => {
                let data: Vec<u16> = command.encode_utf16().chain([0]).collect();
                // SAFETY: `data` is a NUL-terminated UTF-16 string that lives
                // for the call, and the size is its length in bytes.
                unsafe {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        RUN_KEY,
                        &name,
                        REG_SZ.0,
                        Some(data.as_ptr().cast()),
                        (data.len() * size_of::<u16>()) as u32,
                    )
                }
            }
            // SAFETY: the key and value names are NUL-terminated strings.
            None => match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &name) } {
                ERROR_FILE_NOT_FOUND => return Ok(()),
                status => status,
            },
        };
        status.ok()?;
        Ok(())
    }

    pub fn is_set() -> bool {
        let name = HSTRING::from(super::NAME);
        // SAFETY: only the value's presence is asked for; nothing is written.
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                &name,
                RRF_RT_REG_SZ,
                None,
                None,
                None,
            )
        }
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_command_quotes_the_executable() {
        assert_eq!(
            run_command(Path::new(r"C:\Program Files\Launcher\launcher.exe")),
            r#""C:\Program Files\Launcher\launcher.exe" start --background"#
        );
    }

    #[test]
    fn test_launch_agent() {
        assert_eq!(
            launch_agent_path(Path::new("/Users/me")),
            Path::new("/Users/me/Library/LaunchAgents/com.gpui-kit.launcher.plist")
        );
        let agent = launch_agent(Path::new("/Applications/A & B.app/launcher")).unwrap();
        let value = plist::Value::from_reader_xml(agent.as_bytes()).unwrap();
        let agent = value.as_dictionary().unwrap();
        assert_eq!(
            agent.get("Label").and_then(plist::Value::as_string),
            Some(LABEL)
        );
        let arguments: Vec<&str> = agent
            .get("ProgramArguments")
            .and_then(plist::Value::as_array)
            .unwrap()
            .iter()
            .filter_map(plist::Value::as_string)
            .collect();
        assert_eq!(
            arguments,
            ["/Applications/A & B.app/launcher", "start", "--background"]
        );
        assert_eq!(
            agent.get("RunAtLoad").and_then(plist::Value::as_boolean),
            Some(true)
        );
    }

    #[test]
    fn test_autostart_entry() {
        assert_eq!(
            autostart_path(Path::new("/home/me/.config")),
            Path::new("/home/me/.config/autostart/gpui-kit-launcher.desktop")
        );
        let entry = autostart_entry(Path::new("/opt/my apps/launcher"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nName=GPUI Kit Launcher\n"));
        assert!(entry.contains("\nExec=\"/opt/my apps/launcher\" start --background\n"));
        assert!(
            autostart_entry(Path::new("/opt/$x/launcher"))
                .contains("\nExec=\"/opt/\\\\$x/launcher\" start --background\n"),
            "a reserved character is escaped, and the escape escaped again"
        );
    }
}
