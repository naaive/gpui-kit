//! Windows applications: the Start Menu's shortcuts.
//!
//! The Start Menu is a folder of `.lnk` files, per user and for all users.
//! Opening a shortcut with the shell starts its target with the arguments and
//! working directory the installer chose, so the shortcut itself is launched
//! and never parsed.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Application, Launch};

/// The user's Start Menu programs, then the one shared by all users.
#[cfg(target_os = "windows")]
pub fn default_directories() -> Vec<PathBuf> {
    ["APPDATA", "ProgramData"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| {
            PathBuf::from(base)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
        })
        .collect()
}

/// Every shortcut under `directories`, named after its file. A name seen in
/// an earlier directory shadows the same name later: installers put the same
/// shortcut in both menus. Uninstallers are left out; they are reached from
/// Settings, not searched for.
pub fn scan(directories: &[PathBuf]) -> Vec<Application> {
    let mut seen = HashSet::new();
    let mut shortcuts = Vec::new();
    for directory in directories {
        let mut found = Vec::new();
        collect_shortcuts(directory, &mut found);
        found.sort();
        for path in found {
            let Some(name) = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            if name.to_lowercase().contains("uninstall") || !seen.insert(name.to_lowercase()) {
                continue;
            }
            let folder = path
                .parent()
                .filter(|parent| *parent != directory.as_path())
                .and_then(Path::file_name)
                .map(|folder| folder.to_string_lossy().into_owned());
            let application = Application::new(name, path, Launch::Open);
            shortcuts.push(match folder {
                Some(folder) => application.with_subtitle(folder),
                None => application,
            });
        }
    }
    shortcuts
}

fn collect_shortcuts(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            collect_shortcuts(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_shortcuts_and_skips_uninstallers_and_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let common = root.path().join("common");
        for path in [
            user.join("Notepad++.lnk"),
            user.join("Git/Git Bash.lnk"),
            user.join("Git/Uninstall Git.lnk"),
            common.join("notepad++.LNK"),
            common.join("Paint.lnk"),
            common.join("readme.txt"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        }

        let applications = scan(&[user, common]);
        let names: Vec<(&str, Option<&str>)> = applications
            .iter()
            .map(|app| (app.name.as_ref(), app.subtitle.as_ref().map(|s| s.as_ref())))
            .collect();
        assert_eq!(
            names,
            [
                ("Git Bash", Some("Git")),
                ("Notepad++", None),
                ("Paint", None)
            ]
        );
        assert_eq!(applications[2].launch, Launch::Open);
    }
}
