//! Commands for the computer and for the launcher itself.
//!
//! Every operating-system command runs the platform's own tool (`pmset`,
//! `loginctl`, `rundll32`, …) rather than reimplementing it, and is offered
//! only where that tool exists. Commands that end the session or discard data
//! ask first.

use gpui_kit::{
    SharedString,
    component::{ActiveTheme as _, Theme, ThemeMode},
};

use super::{CommandSource, process::CommandLine};
use crate::{
    model::{
        Accessory, Action, ActionStyle, Confirmation, Effect, Item, ItemId, PushHandler, RunHandler,
    },
    shell::settings::settings_page,
};

/// The launcher's own commands, and the operating system's when enabled.
pub struct SystemCommands {
    platform: Vec<SystemCommand>,
}

impl SystemCommands {
    /// With `platform` false only the launcher's own commands are offered,
    /// which keeps tests independent of the machine they run on.
    pub fn new(platform: bool) -> Self {
        Self {
            platform: match platform {
                true => platform_commands(),
                false => Vec::new(),
            },
        }
    }
}

impl CommandSource for SystemCommands {
    fn title(&self) -> SharedString {
        "System".into()
    }

    fn commands(&self) -> Vec<Item> {
        self.platform
            .iter()
            .map(SystemCommand::item)
            .chain(launcher_commands())
            .chain(match self.platform.is_empty() {
                true => Vec::new(),
                false => feature_commands(),
            })
            .chain(match self.platform.is_empty() {
                true => Vec::new(),
                false => crate::window_layout::commands(),
            })
            .chain(match self.platform.is_empty() {
                true => Vec::new(),
                false => settings_pages(),
            })
            .collect()
    }
}

fn launcher_commands() -> [Item; 14] {
    [
        command_item("system/search-files", "Search Files", "file-search")
            .with_keyword("find")
            .with_keyword("documents")
            .with_action(Action::new(
                "Search Files",
                Effect::Push(PushHandler::new(crate::file_search::search_files_page)),
            )),
        command_item("system/create-quicklink", "Create Quicklink", "link")
            .with_keyword("bookmark")
            .with_keyword("url")
            .with_action(Action::new(
                "Create Quicklink",
                Effect::Push(PushHandler::new(crate::quicklinks::create_quicklink_page)),
            )),
        command_item("system/search-quicklinks", "Search Quicklinks", "bookmark")
            .with_keyword("bookmark")
            .with_action(Action::new(
                "Search Quicklinks",
                Effect::Push(PushHandler::new(crate::quicklinks::search_quicklinks_page)),
            )),
        command_item(
            "system/create-snippet",
            "Create Snippet",
            "text-cursor-input",
        )
        .with_keyword("text expansion")
        .with_action(Action::new(
            "Create Snippet",
            Effect::Push(PushHandler::new(crate::snippets::create_snippet_page)),
        )),
        command_item(
            "system/search-snippets",
            "Search Snippets",
            "text-cursor-input",
        )
        .with_keyword("text expansion")
        .with_action(Action::new(
            "Search Snippets",
            Effect::Push(PushHandler::new(crate::snippets::search_snippets_page)),
        )),
        command_item("system/search-processes", "Search Processes", "cpu")
            .with_keyword("kill")
            .with_keyword("quit application")
            .with_keyword("task manager")
            .with_action(Action::new(
                "Search Processes",
                Effect::Push(PushHandler::new(crate::processes::search_processes_page)),
            )),
        command_item(
            "system/create-script-command",
            "Create Script Command",
            "square-terminal",
        )
        .with_keyword("script")
        .with_action(Action::new(
            "Create Script Command",
            Effect::Push(PushHandler::new(
                crate::script_commands::create_script_command,
            )),
        )),
        command_item(
            "system/script-commands-folder",
            "Open Script Commands Folder",
            "folder-open",
        )
        .with_keyword("script")
        .with_action(Action::new(
            "Open Folder",
            Effect::Run(RunHandler::new(|(), _, cx| {
                if let Some(directory) = crate::script_commands::directory() {
                    std::fs::create_dir_all(&directory).ok();
                    crate::shell::launcher::perform(Effect::OpenPath(directory), cx);
                }
            })),
        )),
        command_item("system/search-bookmarks", "Search Bookmarks", "bookmark")
            .with_keyword("browser")
            .with_keyword("chrome")
            .with_keyword("edge")
            .with_action(Action::new(
                "Search Bookmarks",
                Effect::Push(PushHandler::new(crate::bookmarks::search_bookmarks_page)),
            )),
        command_item(
            "system/clipboard-history",
            "Clipboard History",
            "clipboard-list",
        )
        .with_keyword("paste")
        .with_keyword("copy")
        .with_keyword("pasteboard")
        .with_action(Action::new(
            "Open Clipboard History",
            Effect::Push(PushHandler::new(crate::clipboard::clipboard_history_page)),
        )),
        command_item("system/toggle-appearance", "Toggle Appearance", "sun-moon")
            .with_keyword("dark mode")
            .with_keyword("light mode")
            .with_keyword("theme")
            .with_action(Action::new(
                "Toggle Appearance",
                Effect::Run(RunHandler::new(|(), window, cx| {
                    let mode = match cx.theme().is_dark() {
                        true => ThemeMode::Light,
                        false => ThemeMode::Dark,
                    };
                    Theme::change(mode, Some(window), cx);
                })),
            )),
        command_item("system/settings", "Launcher Settings", "settings")
            .with_keyword("preferences")
            .with_action(Action::new(
                "Open Settings",
                Effect::Push(PushHandler::new(settings_page)),
            )),
        command_item("system/extensions", "Manage Extensions", "layout-dashboard")
            .with_keyword("install")
            .with_keyword("plugins")
            .with_action(Action::new(
                "Open Extensions",
                Effect::Push(PushHandler::new(crate::shell::launcher::extensions_page)),
            )),
        command_item("system/quit", "Quit Launcher", "circle-x")
            .with_keyword("exit")
            .with_action(Action::new(
                "Quit",
                Effect::Run(RunHandler::new(|(), _, cx| cx.quit())),
            )),
    ]
}

/// The commands that read the machine: its screen, windows, calendars and
/// files. Like the platform's own, they are left out of tests.
fn feature_commands() -> Vec<Item> {
    let mut commands = Vec::new();
    commands.push(
        command_item(
            "system/search-emoji",
            "Search Emoji & Symbols",
            "face-slightly-smiling",
        )
        .with_keyword("emoji")
        .with_keyword("symbols")
        .with_keyword("character")
        .with_keyword("unicode")
        .with_action(Action::new(
            "Search Emoji & Symbols",
            Effect::Push(PushHandler::new(crate::emoji::search_emoji_page)),
        )),
    );
    commands.extend(crate::colors::commands());
    commands.extend(crate::notes::commands());
    commands.extend(crate::calendar::commands());
    commands.extend(crate::focus::commands());
    commands.extend(crate::shell::backup::commands());
    commands.extend(crate::themes::commands());
    commands.push(
        command_item("system/search-screenshots", "Search Screenshots", "image")
            .with_keyword("screenshot")
            .with_keyword("ocr")
            .with_keyword("capture")
            .with_action(Action::new(
                "Search Screenshots",
                Effect::Push(PushHandler::new(
                    crate::screenshots::search_screenshots_page,
                )),
            )),
    );
    if crate::switch_windows::is_supported() {
        commands.push(
            command_item("system/switch-windows", "Switch Windows", "app-window")
                .with_keyword("alt tab")
                .with_keyword("windows")
                .with_keyword("focus")
                .with_action(Action::new(
                    "Switch Windows",
                    Effect::Push(PushHandler::new(crate::switch_windows::switch_windows_page)),
                )),
        );
    }
    commands
}

/// The extension id deep links use for the launcher's own commands:
/// `launcher://extensions/launcher/clipboard-history` opens
/// the `system/clipboard-history` command.
pub const BUILT_IN_EXTENSION: &str = "launcher";

/// What opening the built-in command `name` does, for a deep link. `name`
/// is a full id (`settings/display`, the deep link spells it
/// `settings%2Fdisplay`) or, for the launcher's own and window commands, the
/// part after `system/` or `window/`.
pub fn built_in_command(name: &str) -> Option<Effect> {
    let commands = SystemCommands::new(true).commands();
    let ids = match name.contains('/') {
        true => vec![name.to_owned()],
        false => vec![format!("system/{name}"), format!("window/{name}")],
    };
    ids.iter()
        .find_map(|id| commands.iter().find(|item| item.id().as_str() == id))
        .and_then(|item| item.primary_action().map(|action| action.effect().clone()))
}

/// The system settings' own pages, opened directly.
fn settings_pages() -> Vec<Item> {
    #[cfg(target_os = "windows")]
    const PAGES: &[(&str, &str, &str, &[&str])] = &[
        (
            "settings",
            "System Settings",
            "settings",
            &["control panel", "preferences"],
        ),
        (
            "display",
            "Display Settings",
            "monitor",
            &["resolution", "brightness", "scale"],
        ),
        (
            "sound",
            "Sound Settings",
            "volume-2",
            &["audio", "speaker", "microphone"],
        ),
        ("bluetooth", "Bluetooth Settings", "bluetooth", &["devices"]),
        (
            "network-wifi",
            "Wi-Fi Settings",
            "wifi",
            &["wireless", "network"],
        ),
        (
            "network",
            "Network Settings",
            "network",
            &["internet", "ethernet", "vpn"],
        ),
        (
            "appsfeatures",
            "Installed Apps",
            "layout-grid",
            &["uninstall", "programs"],
        ),
        ("defaultapps", "Default Apps", "app-window", &["open with"]),
        (
            "windowsupdate",
            "Windows Update",
            "refresh-cw",
            &["updates"],
        ),
        (
            "storagesense",
            "Storage Settings",
            "hard-drive",
            &["disk", "space"],
        ),
        (
            "powersleep",
            "Power Settings",
            "battery",
            &["battery", "sleep"],
        ),
        (
            "personalization",
            "Personalization",
            "palette",
            &["wallpaper", "background", "theme"],
        ),
        (
            "dateandtime",
            "Date & Time Settings",
            "clock",
            &["time zone", "clock"],
        ),
        (
            "keyboard",
            "Keyboard Settings",
            "keyboard",
            &["input", "language"],
        ),
        (
            "notifications",
            "Notification Settings",
            "bell",
            &["focus", "do not disturb"],
        ),
        ("privacy", "Privacy Settings", "shield", &["permissions"]),
    ];
    #[cfg(not(target_os = "windows"))]
    const PAGES: &[(&str, &str, &str, &[&str])] = &[];
    PAGES
        .iter()
        .map(|(page, title, icon, keywords)| {
            let url = match *page {
                "settings" => "ms-settings:".to_owned(),
                page => format!("ms-settings:{page}"),
            };
            keywords.iter().fold(
                Item::new(ItemId::new(format!("settings/{page}")), *title)
                    .with_icon(*icon)
                    .with_accessory(Accessory::text("Settings"))
                    .with_keyword("settings")
                    .with_action(Action::new("Open Settings", Effect::OpenUrl(url.into()))),
                |item, keyword| item.with_keyword(*keyword),
            )
        })
        .collect()
}

fn command_item(id: &'static str, title: &'static str, icon: &'static str) -> Item {
    Item::new(ItemId::new(id), title)
        .with_icon(icon)
        .with_accessory(Accessory::text("Command"))
}

/// An operating-system command: which tool runs it and whether to ask first.
#[derive(Clone, Debug)]
struct SystemCommand {
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    keywords: &'static [&'static str],
    command: CommandLine,
    /// The confirmation's title and its confirm button; `None` runs at once.
    confirmation: Option<(&'static str, &'static str)>,
}

impl SystemCommand {
    fn new(
        id: &'static str,
        title: &'static str,
        icon: &'static str,
        command: CommandLine,
    ) -> Self {
        Self {
            id,
            title,
            icon,
            keywords: &[],
            command,
            confirmation: None,
        }
    }

    fn with_keywords(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }

    /// Asks before running, with a destructive confirm button: the command
    /// ends the session or discards data.
    fn with_confirmation(mut self, title: &'static str, confirm: &'static str) -> Self {
        self.confirmation = Some((title, confirm));
        self
    }

    fn item(&self) -> Item {
        let run = self
            .command
            .clone()
            .effect(format!("Couldn’t run “{}”", self.title));
        let (effect, style) = match self.confirmation {
            Some((title, confirm)) => (
                Effect::Confirm(
                    Confirmation::new(title, run)
                        .with_confirm_title(confirm)
                        .destructive(true),
                ),
                ActionStyle::Destructive,
            ),
            None => (run, ActionStyle::Default),
        };
        self.keywords.iter().fold(
            command_item(self.id, self.title, self.icon)
                .with_action(Action::new(self.title, effect).with_style(style)),
            |item, keyword| item.with_keyword(*keyword),
        )
    }
}

#[cfg(target_os = "macos")]
fn platform_commands() -> Vec<SystemCommand> {
    macos_commands()
}

#[cfg(target_os = "linux")]
fn platform_commands() -> Vec<SystemCommand> {
    linux_commands(
        super::process::is_installed,
        std::env::var("XDG_SESSION_ID").ok(),
    )
}

#[cfg(target_os = "windows")]
fn platform_commands() -> Vec<SystemCommand> {
    windows_commands()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn platform_commands() -> Vec<SystemCommand> {
    Vec::new()
}

/// `osascript` talks to System Events and Finder, which is how macOS's own
/// menu commands are scripted; every tool here ships with macOS.
#[cfg(target_os = "macos")]
fn macos_commands() -> Vec<SystemCommand> {
    fn script(source: &str) -> CommandLine {
        CommandLine::new("/usr/bin/osascript")
            .with_argument("-e")
            .with_argument(source)
    }
    vec![
        SystemCommand::new(
            "system/lock",
            "Lock Screen",
            "lock",
            script(
                r#"tell application "System Events" to keystroke "q" using {control down, command down}"#,
            ),
        ),
        SystemCommand::new(
            "system/sleep",
            "Sleep",
            "moon",
            CommandLine::new("/usr/bin/pmset").with_argument("sleepnow"),
        ),
        SystemCommand::new(
            "system/sleep-displays",
            "Sleep Displays",
            "monitor-off",
            CommandLine::new("/usr/bin/pmset").with_argument("displaysleepnow"),
        ),
        SystemCommand::new(
            "system/empty-trash",
            "Empty Trash",
            "trash",
            script(r#"tell application "Finder" to empty trash"#),
        )
        .with_confirmation("Empty the Trash?", "Empty Trash"),
        SystemCommand::new(
            "system/log-out",
            "Log Out",
            "log-out",
            script(r#"tell application "System Events" to log out"#),
        )
        .with_keywords(&["sign out"])
        .with_confirmation("Log out now?", "Log Out"),
        SystemCommand::new(
            "system/restart",
            "Restart",
            "rotate-ccw",
            script(r#"tell application "System Events" to restart"#),
        )
        .with_keywords(&["reboot"])
        .with_confirmation("Restart now?", "Restart"),
        SystemCommand::new(
            "system/shut-down",
            "Shut Down",
            "power",
            script(r#"tell application "System Events" to shut down"#),
        )
        .with_keywords(&["power off", "shutdown"])
        .with_confirmation("Shut down now?", "Shut Down"),
    ]
}

/// systemd's `loginctl` and `systemctl` cover locking and power on nearly
/// every desktop distribution; `gio` empties the freedesktop.org trash.
/// `is_installed` and `session` are parameters so tests do not depend on the
/// machine.
#[cfg(any(target_os = "linux", test))]
fn linux_commands(
    is_installed: impl Fn(&str) -> bool,
    session: Option<String>,
) -> Vec<SystemCommand> {
    let mut commands = Vec::new();
    if is_installed("loginctl") {
        commands.push(SystemCommand::new(
            "system/lock",
            "Lock Screen",
            "lock",
            CommandLine::new("loginctl").with_argument("lock-session"),
        ));
    }
    if is_installed("systemctl") {
        commands.push(SystemCommand::new(
            "system/sleep",
            "Sleep",
            "moon",
            CommandLine::new("systemctl").with_argument("suspend"),
        ));
    }
    if is_installed("gio") {
        commands.push(
            SystemCommand::new(
                "system/empty-trash",
                "Empty Trash",
                "trash",
                CommandLine::new("gio")
                    .with_argument("trash")
                    .with_argument("--empty"),
            )
            .with_confirmation("Empty the Trash?", "Empty Trash"),
        );
    }
    if let Some(session) = session.filter(|_| is_installed("loginctl")) {
        commands.push(
            SystemCommand::new(
                "system/log-out",
                "Log Out",
                "log-out",
                CommandLine::new("loginctl")
                    .with_argument("terminate-session")
                    .with_argument(session),
            )
            .with_keywords(&["sign out"])
            .with_confirmation("Log out now?", "Log Out"),
        );
    }
    if is_installed("systemctl") {
        commands.push(
            SystemCommand::new(
                "system/restart",
                "Restart",
                "rotate-ccw",
                CommandLine::new("systemctl").with_argument("reboot"),
            )
            .with_keywords(&["reboot"])
            .with_confirmation("Restart now?", "Restart"),
        );
        commands.push(
            SystemCommand::new(
                "system/shut-down",
                "Shut Down",
                "power",
                CommandLine::new("systemctl").with_argument("poweroff"),
            )
            .with_keywords(&["power off", "shutdown"])
            .with_confirmation("Shut down now?", "Shut Down"),
        );
    }
    commands
}

/// Tools in `System32`, present on every Windows installation.
#[cfg(target_os = "windows")]
fn windows_commands() -> Vec<SystemCommand> {
    fn powershell(script: &str) -> CommandLine {
        CommandLine::new("powershell.exe")
            .with_argument("-NoProfile")
            .with_argument("-NonInteractive")
            .with_argument("-Command")
            .with_argument(script)
    }
    /// Presses a media key, which the shell handles like the keyboard's.
    fn media_key(code: u8) -> CommandLine {
        powershell(&format!(
            "(New-Object -ComObject WScript.Shell).SendKeys([char]{code})"
        ))
    }
    vec![
        SystemCommand::new(
            "system/toggle-system-appearance",
            "Toggle System Appearance",
            "sun-moon",
            powershell(concat!(
                r"$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'; ",
                "$next = [int](-not (Get-ItemProperty $key).AppsUseLightTheme); ",
                "Set-ItemProperty $key AppsUseLightTheme $next; ",
                "Set-ItemProperty $key SystemUsesLightTheme $next",
            )),
        )
        .with_keywords(&["dark mode", "light mode"]),
        SystemCommand::new(
            "system/show-desktop",
            "Show Desktop",
            "monitor",
            powershell("(New-Object -ComObject Shell.Application).ToggleDesktop()"),
        )
        .with_keywords(&["hide windows"]),
        SystemCommand::new(
            "system/turn-off-display",
            "Turn Off Display",
            "monitor-off",
            powershell(concat!(
                r#"$display = Add-Type -Name Display -PassThru -MemberDefinition '"#,
                r#"[DllImport("user32.dll")] public static extern int "#,
                r#"SendMessage(int h, int m, int w, int l);'; "#,
                // HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, off.
                "$display::SendMessage(0xffff, 0x0112, 0xF170, 2)",
            )),
        )
        .with_keywords(&["sleep displays", "screen off"]),
        SystemCommand::new(
            "system/toggle-mute",
            "Toggle Mute",
            "volume-x",
            media_key(173),
        )
        .with_keywords(&["volume", "sound", "mute"]),
        SystemCommand::new(
            "system/volume-down",
            "Volume Down",
            "volume-1",
            media_key(174),
        )
        .with_keywords(&["sound", "quieter"]),
        SystemCommand::new("system/volume-up", "Volume Up", "volume-2", media_key(175))
            .with_keywords(&["sound", "louder"]),
        SystemCommand::new(
            "system/lock",
            "Lock Screen",
            "lock",
            CommandLine::new("rundll32.exe").with_argument("user32.dll,LockWorkStation"),
        ),
        SystemCommand::new(
            "system/sleep",
            "Sleep",
            "moon",
            CommandLine::new("rundll32.exe").with_argument("powrprof.dll,SetSuspendState 0,1,0"),
        ),
        SystemCommand::new(
            "system/empty-trash",
            "Empty Recycle Bin",
            "trash",
            CommandLine::new("powershell.exe")
                .with_argument("-NoProfile")
                .with_argument("-Command")
                .with_argument("Clear-RecycleBin -Force"),
        )
        .with_keywords(&["trash"])
        .with_confirmation("Empty the Recycle Bin?", "Empty Recycle Bin"),
        SystemCommand::new(
            "system/hibernate",
            "Hibernate",
            "moon-star",
            CommandLine::new("shutdown.exe").with_argument("/h"),
        )
        .with_confirmation("Hibernate now?", "Hibernate"),
        SystemCommand::new(
            "system/log-out",
            "Sign Out",
            "log-out",
            CommandLine::new("shutdown.exe").with_argument("/l"),
        )
        .with_keywords(&["log out"])
        .with_confirmation("Sign out now?", "Sign Out"),
        SystemCommand::new(
            "system/restart",
            "Restart",
            "rotate-ccw",
            CommandLine::new("shutdown.exe")
                .with_argument("/r")
                .with_argument("/t")
                .with_argument("0"),
        )
        .with_keywords(&["reboot"])
        .with_confirmation("Restart now?", "Restart"),
        SystemCommand::new(
            "system/shut-down",
            "Shut Down",
            "power",
            CommandLine::new("shutdown.exe")
                .with_argument("/s")
                .with_argument("/t")
                .with_argument("0"),
        )
        .with_keywords(&["power off", "shutdown"])
        .with_confirmation("Shut down now?", "Shut Down"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(commands: &[SystemCommand]) -> Vec<&str> {
        commands.iter().map(|command| command.id).collect()
    }

    #[test]
    fn test_linux_offers_only_commands_whose_tools_exist() {
        assert!(linux_commands(|_| false, Some("2".into())).is_empty());
        assert_eq!(
            ids(&linux_commands(|tool| tool == "loginctl", None)),
            ["system/lock"],
            "logging out needs the session id"
        );
        assert_eq!(
            ids(&linux_commands(|_| true, Some("2".into()))),
            [
                "system/lock",
                "system/sleep",
                "system/empty-trash",
                "system/log-out",
                "system/restart",
                "system/shut-down"
            ]
        );
    }

    #[test]
    fn test_ending_the_session_or_discarding_data_asks_first() {
        for command in linux_commands(|_| true, Some("2".into())) {
            let item = command.item();
            let action = item.primary_action().unwrap();
            let asks = matches!(action.effect(), Effect::Confirm(_));
            let expected = !matches!(command.id, "system/lock" | "system/sleep");
            assert_eq!(asks, expected, "{}", command.id);
            assert_eq!(
                action.style() == ActionStyle::Destructive,
                expected,
                "{}",
                command.id
            );
        }
    }

    #[test]
    fn test_built_in_commands_by_short_name_or_full_id() {
        assert!(
            matches!(built_in_command("settings"), Some(Effect::Push(_))),
            "the short name is the launcher's own Settings"
        );
        assert!(built_in_command("system/quit").is_some());
        assert!(built_in_command("nothing/here").is_none());
    }

    #[test]
    fn test_launcher_commands_are_always_offered() {
        let titles: Vec<String> = SystemCommands::new(false)
            .commands()
            .iter()
            .map(|item| item.title().to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "Search Files",
                "Create Quicklink",
                "Search Quicklinks",
                "Create Snippet",
                "Search Snippets",
                "Search Processes",
                "Create Script Command",
                "Open Script Commands Folder",
                "Search Bookmarks",
                "Clipboard History",
                "Toggle Appearance",
                "Launcher Settings",
                "Manage Extensions",
                "Quit Launcher"
            ]
        );
    }
}
