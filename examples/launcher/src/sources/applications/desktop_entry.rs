//! Linux applications: XDG desktop entries.
//!
//! Follows the Desktop Entry Specification 1.5 as far as a launcher needs it:
//! desktop file ids and their precedence across data directories, `Hidden`,
//! `NoDisplay`, `OnlyShowIn`/`NotShowIn`, `TryExec`, localized keys, and the
//! `Exec` quoting and field-code rules. Everything here takes its environment
//! as a parameter, so it is tested against temporary directories.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::{Application, Launch};
use crate::sources::process::{CommandLine, is_installed};

/// `$XDG_DATA_HOME/applications`, then each `$XDG_DATA_DIRS/applications`,
/// with the specification's defaults when a variable is unset or empty.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "used by the Linux scan")
)]
pub fn default_directories() -> Vec<PathBuf> {
    data_directories()
        .into_iter()
        .map(|directory| directory.join("applications"))
        .collect()
}

#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "used by the Linux scan")
)]
fn data_directories() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home)
        .chain(
            data_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .collect()
}

/// What an entry is matched against: the user's locale and desktop, and the
/// terminal that runs `Terminal=true` applications.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// `lang_COUNTRY.ENCODING@MODIFIER`, as in `LC_MESSAGES`.
    locale: Option<String>,
    /// `$XDG_CURRENT_DESKTOP`, split at `:`.
    desktops: Vec<String>,
    /// The terminal command a program's arguments are appended to.
    terminal: Option<Vec<String>>,
}

impl Environment {
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "used by the Linux scan")
    )]
    pub fn current() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .filter(|value| value != "C" && value != "POSIX");
        let desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|desktop| !desktop.is_empty())
            .map(str::to_owned)
            .collect();
        Self {
            locale,
            desktops,
            terminal: find_terminal(std::env::var("TERMINAL").ok(), is_installed),
        }
    }

    #[cfg(test)]
    fn new(locale: Option<&str>, desktops: &[&str], terminal: Option<&[&str]>) -> Self {
        Self {
            locale: locale.map(str::to_owned),
            desktops: desktops.iter().map(|d| (*d).to_owned()).collect(),
            terminal: terminal.map(|t| t.iter().map(|a| (*a).to_owned()).collect()),
        }
    }
}

/// `$TERMINAL`, else the first installed well-known terminal, with the flag
/// that makes it run a command.
fn find_terminal(
    preferred: Option<String>,
    is_installed: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    if let Some(terminal) = preferred.filter(|terminal| is_installed(terminal)) {
        return Some(vec![terminal, "-e".into()]);
    }
    const TERMINALS: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("kgx", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-x"]),
        ("alacritty", &["-e"]),
        ("kitty", &[]),
        ("foot", &[]),
        ("wezterm", &["start", "--"]),
        ("xterm", &["-e"]),
    ];
    TERMINALS
        .iter()
        .find(|(terminal, _)| is_installed(terminal))
        .map(|(terminal, flags)| {
            std::iter::once(*terminal)
                .chain(flags.iter().copied())
                .map(str::to_owned)
                .collect()
        })
}

/// Finds icon files by name: `hicolor` first at 48–64 px, then scalable and
/// larger sizes, then `pixmaps`.
#[derive(Clone, Debug, Default)]
pub struct IconLookup {
    /// Directories holding icon themes, such as `/usr/share/icons`.
    themes: Vec<PathBuf>,
    /// Directories holding loose icons, such as `/usr/share/pixmaps`.
    pixmaps: Vec<PathBuf>,
}

const ICON_SIZES: &[&str] = &[
    "48x48", "64x64", "scalable", "96x96", "128x128", "256x256", "32x32",
];

impl IconLookup {
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "used by the Linux scan")
    )]
    pub fn current() -> Self {
        let data = data_directories();
        let home_icons = dirs::home_dir().map(|home| home.join(".icons"));
        Self {
            themes: home_icons
                .into_iter()
                .chain(data.iter().map(|dir| dir.join("icons")))
                .collect(),
            pixmaps: data.iter().map(|dir| dir.join("pixmaps")).collect(),
        }
    }

    #[cfg(test)]
    fn new(themes: Vec<PathBuf>, pixmaps: Vec<PathBuf>) -> Self {
        Self { themes, pixmaps }
    }

    /// The file for an entry's `Icon` value: an absolute path as is, a name
    /// looked up in the themes and pixmaps.
    pub fn find(&self, icon: &str) -> Option<PathBuf> {
        let path = Path::new(icon);
        if path.is_absolute() {
            return path.is_file().then(|| path.to_path_buf());
        }
        let name = icon
            .strip_suffix(".png")
            .or_else(|| icon.strip_suffix(".svg"))
            .unwrap_or(icon);
        let themed = self.themes.iter().flat_map(|base| {
            ICON_SIZES.iter().flat_map(move |size| {
                ["png", "svg"].into_iter().map(move |extension| {
                    base.join("hicolor")
                        .join(size)
                        .join("apps")
                        .join(format!("{name}.{extension}"))
                })
            })
        });
        let loose = self.pixmaps.iter().flat_map(|directory| {
            ["png", "svg"]
                .into_iter()
                .map(move |extension| directory.join(format!("{name}.{extension}")))
        });
        themed.chain(loose).find(|candidate| candidate.is_file())
    }
}

/// Reads every visible application under `directories`.
///
/// A desktop file id seen in an earlier directory shadows the same id later,
/// even when the earlier entry is hidden: that is how a user removes a system
/// application from their menus.
pub fn scan(
    directories: &[PathBuf],
    environment: &Environment,
    icons: &IconLookup,
) -> Vec<Application> {
    let mut seen = HashSet::new();
    let mut applications = Vec::new();
    for directory in directories {
        let mut files = Vec::new();
        collect_desktop_files(directory, directory, &mut files);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(application) = parse(&source, &id, &path, environment, icons) {
                applications.push(application);
            }
        }
    }
    applications
}

/// Desktop files below `directory` with their ids: the path relative to the
/// `applications` directory, with `/` replaced by `-`.
fn collect_desktop_files(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(root, &path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
            && let Ok(relative) = path.strip_prefix(root)
        {
            let id = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("-");
            files.push((id, path));
        }
    }
}

/// The `[Desktop Entry]` group's keys, with values unescaped.
struct Entry {
    keys: HashMap<String, String>,
}

impl Entry {
    fn read(source: &str) -> Self {
        let mut keys = HashMap::new();
        let mut in_entry = false;
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                keys.entry(key.trim().to_owned())
                    .or_insert_with(|| value.trim_start().to_owned());
            }
        }
        Self { keys }
    }

    fn raw(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }

    fn string(&self, key: &str) -> Option<String> {
        self.raw(key).map(unescape)
    }

    fn boolean(&self, key: &str) -> bool {
        self.raw(key) == Some("true")
    }

    /// A localized key's value for `locale`, trying `lang_COUNTRY@MODIFIER`,
    /// `lang_COUNTRY`, `lang@MODIFIER`, `lang`, then the unlocalized key.
    fn localized(&self, key: &str, locale: Option<&str>) -> Option<String> {
        locale
            .into_iter()
            .flat_map(locale_variants)
            .find_map(|variant| self.string(&format!("{key}[{variant}]")))
            .or_else(|| self.string(key))
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.raw(key).map(split_list).unwrap_or_default()
    }

    fn localized_list(&self, key: &str, locale: Option<&str>) -> Vec<String> {
        locale
            .into_iter()
            .flat_map(locale_variants)
            .find_map(|variant| self.raw(&format!("{key}[{variant}]")))
            .or_else(|| self.raw(key))
            .map(split_list)
            .unwrap_or_default()
    }
}

/// The lookup order of the specification's "Localized values for keys".
fn locale_variants(locale: &str) -> Vec<String> {
    let (rest, modifier) = match locale.split_once('@') {
        Some((rest, modifier)) => (rest, Some(modifier)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or(rest);
    let (language, country) = match rest.split_once('_') {
        Some((language, country)) => (language, Some(country)),
        None => (rest, None),
    };
    let mut variants = Vec::new();
    if let (Some(country), Some(modifier)) = (country, modifier) {
        variants.push(format!("{language}_{country}@{modifier}"));
    }
    if let Some(country) = country {
        variants.push(format!("{language}_{country}"));
    }
    if let Some(modifier) = modifier {
        variants.push(format!("{language}@{modifier}"));
    }
    variants.push(language.to_owned());
    variants
}

/// The `\s \n \t \r \\` escapes of string values.
fn unescape(value: &str) -> String {
    let mut unescaped = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        match chars.next() {
            Some('s') => unescaped.push(' '),
            Some('n') => unescaped.push('\n'),
            Some('t') => unescaped.push('\t'),
            Some('r') => unescaped.push('\r'),
            Some('\\') => unescaped.push('\\'),
            // Other escapes belong to a later stage: `\;` to lists, `\"` and
            // `\$` to `Exec` quoting.
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }
    unescaped
}

/// A `;`-separated list, where `\;` is a literal semicolon.
fn split_list(value: &str) -> Vec<String> {
    let value = unescape(value);
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if chars.peek() == Some(&';') => {
                current.push(';');
                chars.next();
            }
            ';' => items.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    items.push(current);
    items
        .into_iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Reads one desktop file into an application, or `None` when it is not a
/// visible application in this environment.
fn parse(
    source: &str,
    id: &str,
    path: &Path,
    environment: &Environment,
    icons: &IconLookup,
) -> Option<Application> {
    let entry = Entry::read(source);
    if entry.raw("Type") != Some("Application")
        || entry.boolean("Hidden")
        || entry.boolean("NoDisplay")
    {
        return None;
    }
    let shown_in = |key: &str| {
        entry.list(key).iter().any(|desktop| {
            environment
                .desktops
                .iter()
                .any(|current| current == desktop)
        })
    };
    if (entry.raw("OnlyShowIn").is_some() && !shown_in("OnlyShowIn")) || shown_in("NotShowIn") {
        return None;
    }
    if let Some(try_exec) = entry.string("TryExec")
        && !is_installed(&try_exec)
    {
        return None;
    }

    let locale = environment.locale.as_deref();
    let name = entry.localized("Name", locale)?;
    let icon = entry.string("Icon").filter(|icon| !icon.is_empty());
    let arguments = expand_exec(
        &entry.string("Exec")?,
        &name,
        icon.as_deref(),
        &path.to_string_lossy(),
    )?;
    let arguments = match (entry.boolean("Terminal"), &environment.terminal) {
        (true, Some(terminal)) => terminal.iter().cloned().chain(arguments).collect(),
        _ => arguments,
    };
    let (program, arguments) = arguments.split_first()?;
    let command = CommandLine::new(program.clone()).with_arguments(arguments.iter().cloned());
    let command = match entry.string("Path").filter(|path| !path.is_empty()) {
        Some(directory) => command.with_working_directory(PathBuf::from(directory)),
        None => command,
    };

    let application = Application::new(name, path.to_path_buf(), Launch::Run(command))
        .with_id(format!("app:{}", id.trim_end_matches(".desktop")));
    let application = match entry.localized("GenericName", locale) {
        Some(generic) => application.with_subtitle(generic),
        None => application,
    };
    let application = match icon.as_deref().and_then(|icon| icons.find(icon)) {
        Some(icon) => application.with_icon(icon),
        None => application,
    };
    // The untranslated name stays searchable, so `settings` still finds
    // "设置" on a Chinese desktop.
    Some(
        entry
            .string("Name")
            .into_iter()
            .chain(entry.localized_list("Keywords", locale))
            .fold(application, Application::with_keyword),
    )
}

/// Splits an `Exec` value into arguments and expands its field codes for a
/// launch with no files: `%f %F %u %U` vanish, `%i` becomes `--icon <Icon>`,
/// `%c` the name, `%k` the desktop file, `%%` a percent sign.
///
/// `None` when the value is malformed (an unterminated quote), which the
/// specification says to reject rather than guess at.
fn expand_exec(exec: &str, name: &str, icon: Option<&str>, file: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    for argument in split_exec(exec)? {
        match argument.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            "%i" => {
                if let Some(icon) = icon {
                    arguments.push("--icon".to_owned());
                    arguments.push(icon.to_owned());
                }
            }
            _ => arguments.push(expand_codes(&argument, name, file)),
        }
    }
    (!arguments.is_empty()).then_some(arguments)
}

/// Field codes inside a longer argument, such as `--name=%c`.
fn expand_codes(argument: &str, name: &str, file: &str) -> String {
    let mut expanded = String::with_capacity(argument.len());
    let mut chars = argument.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            expanded.push(character);
            continue;
        }
        match chars.next() {
            Some('%') => expanded.push('%'),
            Some('c') => expanded.push_str(name),
            Some('k') => expanded.push_str(file),
            // File and URL codes have nothing to expand to, and deprecated
            // or unknown codes are dropped.
            _ => {}
        }
    }
    expanded
}

/// Splits at unquoted spaces; inside double quotes, `\"`, `` \` ``, `\$` and
/// `\\` stand for the escaped character.
fn split_exec(exec: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut in_argument = false;
    let mut chars = exec.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => {
                in_argument = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '`' | '$' | '\\') => current.push(escaped),
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        },
                        other => current.push(other),
                    }
                }
            }
            ' ' | '\t' => {
                if in_argument {
                    arguments.push(std::mem::take(&mut current));
                    in_argument = false;
                }
            }
            _ => {
                current.push(character);
                in_argument = true;
            }
        }
    }
    if in_argument {
        arguments.push(current);
    }
    Some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, source: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }

    fn command(application: &Application) -> (&str, Vec<&str>) {
        let Launch::Run(command) = &application.launch else {
            panic!("a desktop entry runs a command");
        };
        (
            command.program(),
            command.arguments().iter().map(String::as_str).collect(),
        )
    }

    #[test]
    fn test_exec_quoting_and_field_codes() {
        assert_eq!(
            expand_exec(
                r#"firefox %u --class="Fire Fox" "a \"b\" \$c" --name=%c %%x %i %k"#,
                "Firefox",
                Some("firefox"),
                "/apps/firefox.desktop"
            )
            .unwrap(),
            [
                "firefox",
                "--class=Fire Fox",
                r#"a "b" $c"#,
                "--name=Firefox",
                "%x",
                "--icon",
                "firefox",
                "/apps/firefox.desktop"
            ]
        );
        assert_eq!(expand_exec("app %F", "App", None, "").unwrap(), ["app"]);
        assert_eq!(expand_exec("app %i", "App", None, "").unwrap(), ["app"]);
        assert!(expand_exec(r#"app "unterminated"#, "App", None, "").is_none());
        assert!(expand_exec("%U", "App", None, "").is_none());
    }

    #[test]
    fn test_locale_variants_follow_the_specification() {
        assert_eq!(
            locale_variants("sr_YU.UTF-8@Latn"),
            ["sr_YU@Latn", "sr_YU", "sr@Latn", "sr"]
        );
        assert_eq!(locale_variants("zh_CN.UTF-8"), ["zh_CN", "zh"]);
        assert_eq!(split_list(r"a;b\;c;;d;"), ["a", "b;c", "d"]);
    }

    #[test]
    fn test_scans_visible_applications_in_precedence_order() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("home/applications");
        let system = root.path().join("usr/applications");
        let icons = root.path().join("icons");
        let pixmaps = root.path().join("pixmaps");
        write(&icons.join("hicolor/48x48/apps/wechat.png"), "png");
        write(&pixmaps.join("gedit.svg"), "svg");

        write(
            &system.join("wechat.desktop"),
            "[Desktop Entry]\nType=Application\nName=WeChat\nName[zh_CN]=微信\n\
             GenericName=Messenger\nGenericName[zh]=聊天\nKeywords=chat;im;\n\
             Keywords[zh_CN]=聊天;\nExec=/opt/wechat/wechat %U\nIcon=wechat\n\
             [Desktop Action New]\nName=Ignored\nExec=ignored\n",
        );
        write(
            &system.join("org.gnome/gedit.desktop"),
            "[Desktop Entry]\nType=Application\nName=Text Editor\nExec=gedit %F\n\
             Icon=gedit\nPath=/tmp\n",
        );
        write(
            &system.join("htop.desktop"),
            "[Desktop Entry]\nType=Application\nName=htop\nExec=htop\nTerminal=true\n",
        );
        // Filtered out: a link, a hidden entry, one only for another desktop,
        // one excluded from this desktop, one that fails `TryExec`.
        write(
            &system.join("site.desktop"),
            "[Desktop Entry]\nType=Link\nName=Site\nURL=https://gpui-kit.com\n",
        );
        write(
            &system.join("helper.desktop"),
            "[Desktop Entry]\nType=Application\nName=Helper\nExec=helper\nNoDisplay=true\n",
        );
        write(
            &system.join("kde-only.desktop"),
            "[Desktop Entry]\nType=Application\nName=KDE\nExec=k\nOnlyShowIn=KDE;\n",
        );
        write(
            &system.join("not-gnome.desktop"),
            "[Desktop Entry]\nType=Application\nName=Not GNOME\nExec=n\nNotShowIn=GNOME;\n",
        );
        write(
            &system.join("missing.desktop"),
            "[Desktop Entry]\nType=Application\nName=Missing\nExec=m\n\
             TryExec=/nonexistent/launcher-test-binary\n",
        );
        // The user's copy of `removed.desktop` hides the system one.
        write(
            &system.join("removed.desktop"),
            "[Desktop Entry]\nType=Application\nName=Removed\nExec=r\n",
        );
        write(
            &user.join("removed.desktop"),
            "[Desktop Entry]\nType=Application\nName=Removed\nExec=r\nHidden=true\n",
        );
        write(
            &system.join("gnome-only.desktop"),
            "[Desktop Entry]\nType=Application\nName=GNOME Tool\nExec=g\n\
             OnlyShowIn=Unity;GNOME;\n",
        );

        let environment = Environment::new(
            Some("zh_CN.UTF-8"),
            &["ubuntu", "GNOME"],
            Some(&["xterm", "-e"]),
        );
        let lookup = IconLookup::new(vec![icons.clone()], vec![pixmaps.clone()]);
        let applications = scan(&[user, system.clone()], &environment, &lookup);
        let ids: Vec<&str> = applications.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "app:gnome-only",
                "app:htop",
                "app:org.gnome-gedit",
                "app:wechat"
            ]
        );

        let wechat = &applications[3];
        assert_eq!(wechat.name.as_ref(), "微信");
        assert_eq!(wechat.subtitle.as_ref().map(|s| s.as_ref()), Some("聊天"));
        assert_eq!(wechat.keywords, ["WeChat", "聊天"]);
        assert_eq!(
            wechat.icon.as_deref(),
            Some(icons.join("hicolor/48x48/apps/wechat.png").as_path())
        );
        assert_eq!(command(wechat), ("/opt/wechat/wechat", vec![]));

        let gedit = &applications[2];
        assert_eq!(
            gedit.icon.as_deref(),
            Some(pixmaps.join("gedit.svg").as_path())
        );
        let Launch::Run(gedit_command) = &gedit.launch else {
            unreachable!()
        };
        assert_eq!(
            gedit_command.working_directory(),
            Some(&PathBuf::from("/tmp"))
        );

        assert_eq!(
            command(&applications[1]),
            ("xterm", vec!["-e", "htop"]),
            "a terminal application runs inside the terminal"
        );

        // In English the untranslated values are used.
        let english = Environment::new(Some("en_US.UTF-8"), &["GNOME"], None);
        let applications = scan(&[system], &english, &lookup);
        let wechat = applications
            .iter()
            .find(|application| application.id == "app:wechat")
            .unwrap();
        assert_eq!(wechat.name.as_ref(), "WeChat");
        assert_eq!(wechat.keywords, ["chat", "im"]);
    }

    #[test]
    fn test_finds_a_terminal() {
        assert_eq!(
            find_terminal(Some("wezterm".into()), |_| true),
            Some(vec!["wezterm".to_owned(), "-e".to_owned()])
        );
        assert_eq!(
            find_terminal(None, |terminal| terminal == "konsole"),
            Some(vec!["konsole".to_owned(), "-e".to_owned()])
        );
        assert_eq!(find_terminal(None, |_| false), None);
    }
}
