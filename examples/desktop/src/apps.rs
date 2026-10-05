//! Installed applications, read from freedesktop desktop entries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::SharedString;

use crate::compositor::WindowIdentity;

/// Applications pinned to the dock when the user has not chosen any, in order.
/// Only the installed ones appear.
const DEFAULT_FAVORITES: &[&str] = &[
    "firefox",
    "org.mozilla.firefox",
    "chromium",
    "google-chrome",
    "org.gnome.Nautilus",
    "org.kde.dolphin",
    "thunar",
    "org.gnome.Console",
    "org.gnome.Terminal",
    "org.kde.konsole",
    "org.codeberg.dnkl.foot",
    "Alacritty",
    "kitty",
    "dev.zed.Zed",
    "code",
    "org.gnome.TextEditor",
    "libreoffice-writer",
    "libreoffice-startcenter",
    "org.gnome.Settings",
];

const ICON_THEMES: &[&str] = &["Adwaita", "breeze", "Papirus", "hicolor"];
const ICON_SIZES: &[&str] = &[
    "scalable", "512x512", "256x256", "128x128", "96x96", "64x64", "48x48",
];

#[derive(Clone, Debug)]
pub struct DesktopApp {
    id: SharedString,
    name: SharedString,
    icon: Option<PathBuf>,
    command_line: String,
    window_class: Option<String>,
}

impl DesktopApp {
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn icon(&self) -> Option<&PathBuf> {
        self.icon.as_ref()
    }

    /// The `Exec` line with its field codes removed, ready for `sh -c`.
    pub fn command_line(&self) -> &str {
        &self.command_line
    }

    /// Whether a window belongs to this application: by desktop ID, the
    /// convention for Wayland `app_id`s, or by the entry's `StartupWMClass`.
    pub fn owns(&self, window: &WindowIdentity) -> bool {
        let name = window.name();
        name.eq_ignore_ascii_case(&self.id)
            || self
                .window_class
                .as_deref()
                .is_some_and(|class| name.eq_ignore_ascii_case(class))
            || self
                .id
                .rsplit('.')
                .next()
                .is_some_and(|last| name.eq_ignore_ascii_case(last))
    }
}

/// Every launchable application, keyed by desktop ID.
pub struct AppCatalog {
    apps: Vec<DesktopApp>,
}

impl AppCatalog {
    pub fn load() -> Self {
        let mut by_id = HashMap::new();
        // Earlier directories take precedence, as the spec requires, so walk
        // them last to first and let the earlier entry overwrite.
        for dir in data_dirs().iter().rev() {
            let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "desktop")
                    && let Some(app) = parse_desktop_entry(&path)
                {
                    by_id.insert(app.id.clone(), app);
                }
            }
        }
        let mut apps: Vec<_> = by_id.into_values().collect();
        apps.sort_by_key(|app| app.name.to_lowercase());
        Self { apps }
    }

    /// The user's favorites, one desktop ID per line in
    /// `$XDG_CONFIG_HOME/gpui-kit-desktop/favorites`, else the installed
    /// defaults.
    pub fn favorites(&self) -> Vec<DesktopApp> {
        let configured = config_home()
            .map(|dir| dir.join("gpui-kit-desktop/favorites"))
            .and_then(|path| std::fs::read_to_string(path).ok());
        let ids: Vec<String> = match configured {
            Some(text) => text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| line.trim_end_matches(".desktop").to_string())
                .collect(),
            None => DEFAULT_FAVORITES.iter().map(|id| id.to_string()).collect(),
        };
        ids.iter().filter_map(|id| self.get(id)).cloned().collect()
    }

    pub fn owner_of(&self, window: &WindowIdentity) -> Option<&DesktopApp> {
        self.apps.iter().find(|app| app.owns(window))
    }

    fn get(&self, id: &str) -> Option<&DesktopApp> {
        self.apps.iter().find(|app| app.id.as_ref() == id)
    }
}

fn parse_desktop_entry(path: &Path) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields = HashMap::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry && let Some((key, value)) = line.split_once('=') {
            fields.insert(key.trim(), value.trim());
        }
    }

    let is_true = |key: &str| fields.get(key).is_some_and(|value| *value == "true");
    if fields.get("Type") != Some(&"Application") || is_true("NoDisplay") || is_true("Hidden") {
        return None;
    }

    Some(DesktopApp {
        id: path.file_stem()?.to_string_lossy().to_string().into(),
        name: SharedString::from(fields.get("Name")?.to_string()),
        icon: fields.get("Icon").and_then(|icon| find_icon(icon)),
        command_line: strip_field_codes(fields.get("Exec")?),
        window_class: fields.get("StartupWMClass").map(|class| class.to_string()),
    })
}

/// Removes `%f`, `%U` and the other field codes, which only matter when the
/// launcher passes files or URLs, and unescapes `%%`.
fn strip_field_codes(exec: &str) -> String {
    let mut command = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            command.push(c);
            continue;
        }
        if chars.next() == Some('%') {
            command.push('%');
        }
    }
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A minimal icon theme lookup: the common themes' application icons, then
/// `pixmaps`. It ignores `index.theme` inheritance and the user's chosen theme.
fn find_icon(icon: &str) -> Option<PathBuf> {
    let path = Path::new(icon);
    if path.is_absolute() {
        return path.exists().then(|| path.to_path_buf());
    }
    let dirs = data_dirs();
    for dir in &dirs {
        for theme in ICON_THEMES {
            for size in ICON_SIZES {
                for ext in ["svg", "png"] {
                    let candidate = dir.join(format!("icons/{theme}/{size}/apps/{icon}.{ext}"));
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    dirs.iter()
        .chain(std::iter::once(&PathBuf::from("/usr/share")))
        .flat_map(|dir| ["svg", "png"].map(|ext| dir.join(format!("pixmaps/{icon}.{ext}"))))
        .find(|candidate| candidate.exists())
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// `$XDG_DATA_HOME` followed by `$XDG_DATA_DIRS`, most important first.
fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(system.split(':').map(PathBuf::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_field_codes_and_keeps_escaped_percent() {
        assert_eq!(strip_field_codes("firefox %u"), "firefox");
        assert_eq!(
            strip_field_codes("env GDK_BACKEND=x11 app --name=%c %F"),
            "env GDK_BACKEND=x11 app --name="
        );
        assert_eq!(strip_field_codes("printf 100%%"), "printf 100%");
    }

    #[test]
    fn matches_windows_by_id_class_and_reverse_dns_tail() {
        let app = DesktopApp {
            id: "org.gnome.Nautilus".into(),
            name: "Files".into(),
            icon: None,
            command_line: "nautilus".into(),
            window_class: Some("Org.gnome.Nautilus-x11".into()),
        };
        assert!(app.owns(&WindowIdentity::AppId("org.gnome.Nautilus".into())));
        assert!(app.owns(&WindowIdentity::Class("org.gnome.nautilus-X11".into())));
        assert!(app.owns(&WindowIdentity::AppId("nautilus".into())));
        assert!(!app.owns(&WindowIdentity::AppId("foot".into())));
    }
}
