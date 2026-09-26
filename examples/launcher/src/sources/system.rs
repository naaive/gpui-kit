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
            .collect()
    }
}

fn launcher_commands() -> [Item; 3] {
    [
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
        command_item("system/quit", "Quit Launcher", "circle-x")
            .with_keyword("exit")
            .with_action(Action::new(
                "Quit",
                Effect::Run(RunHandler::new(|(), _, cx| cx.quit())),
            )),
    ]
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
            .effect(format!("Cannot run “{}”", self.title));
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
    vec![
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
    fn test_launcher_commands_are_always_offered() {
        let titles: Vec<String> = SystemCommands::new(false)
            .commands()
            .iter()
            .map(|item| item.title().to_string())
            .collect();
        assert_eq!(
            titles,
            ["Toggle Appearance", "Launcher Settings", "Quit Launcher"]
        );
    }
}
