//! Starting another program without tying it to the launcher.

use std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
};

use gpui_kit::{
    SharedString,
    component::{
        WindowExt as _,
        notification::{Notification, NotificationType},
    },
};

use crate::model::{Effect, RunHandler};

/// A program and its arguments, as the launcher runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandLine {
    program: String,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
}

impl CommandLine {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            working_directory: None,
        }
    }

    pub fn with_argument(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn with_arguments(mut self, arguments: impl IntoIterator<Item = String>) -> Self {
        self.arguments.extend(arguments);
        self
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn with_working_directory(mut self, directory: PathBuf) -> Self {
        self.working_directory = Some(directory);
        self
    }

    #[cfg(test)]
    pub fn program(&self) -> &str {
        &self.program
    }

    #[cfg(test)]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[cfg(test)]
    pub fn working_directory(&self) -> Option<&PathBuf> {
        self.working_directory.as_ref()
    }

    /// Starts the program detached: its own process group, no inherited
    /// standard streams, so it outlives the launcher and quitting the launcher
    /// never takes it down.
    pub fn spawn_detached(&self) -> io::Result<()> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(directory) = &self.working_directory {
            command.current_dir(directory);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // CREATE_NO_WINDOW: console tools such as `shutdown.exe` would
            // otherwise flash a console window.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        // Reaps the child when it exits, so it never lingers as a zombie.
        std::thread::Builder::new()
            .name("launcher-child".into())
            .spawn(move || child.wait())
            .map(|_| ())
    }

    /// An effect that runs this command, explaining in a notification when it
    /// cannot be started.
    pub fn effect(self, failure: impl Into<SharedString>) -> Effect {
        let failure = failure.into();
        Effect::Run(RunHandler::new(move |(), window, cx| {
            // Starting a program is leaving the launcher, like opening a file.
            if let Err(error) = self
                .spawn_detached()
                .inspect(|_| crate::shell::launcher::hide(cx))
            {
                tracing::warn!("cannot start `{}`: {error}", self.program);
                window.push_notification(
                    Notification::new()
                        .title(failure.clone())
                        .message(format!("{}: {error}", self.program))
                        .with_type(NotificationType::Error),
                    cx,
                );
            }
        }))
    }
}

/// Whether `program` can be run: an existing path, or a name found on `PATH`.
#[cfg(any(target_os = "linux", test))]
pub fn is_installed(program: &str) -> bool {
    let path = std::path::Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(search) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search).any(|directory| {
        let candidate = directory.join(program);
        candidate.is_file() || (cfg!(windows) && candidate.with_extension("exe").is_file())
    })
}
