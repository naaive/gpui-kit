//! Query consoles: a SQL editor with a session of its own, the statements it
//! ran, and their results.

mod completion;
mod console_panel;
mod intelligence;
mod parameters_dialog;
mod sessions;
mod usages;

use gpui_kit::component::input::{GoToDefinition, ToggleCodeActions};
use gpui_kit::{App, KeyBinding, actions};

pub use console_panel::{ConsolePanel, SessionState};
pub use sessions::{Sessions, SessionsEvent};

actions!(
    console,
    [
        /// Run the statement at the caret, or every statement in the selection.
        ExecuteStatement,
        /// Ask the server to stop the running statement.
        CancelExecution,
        /// Show how the database would run the statement at the caret.
        ExplainPlan,
        /// Run the statement at the caret and show how the database ran it.
        ExplainAnalyze,
        /// Lay the console's SQL out again.
        FormatSql,
        /// Commit the console's transaction.
        Commit,
        /// Roll back the console's transaction.
        Rollback,
        /// Show what the name at the caret is.
        QuickDocumentation,
        /// Rename the alias at the caret everywhere in its statement, or the
        /// table, view or column the name at the caret refers to.
        RenameAlias,
        /// Save the console's text to a SQL file and edit that file.
        SaveConsoleAs,
        /// Offer completions for the word at the caret.
        ShowCompletions,
        /// List where the open consoles name the object at the caret.
        FindUsages,
        /// Show the arguments of the routine call the caret is in.
        ParameterInfo,
        /// Show the earlier versions of the console's text.
        ShowLocalHistory
    ]
);

pub(crate) const CONTEXT: &str = "Console";

pub fn init(cx: &mut App) {
    const EDITOR: &str = "Console > Input";
    cx.bind_keys([
        // The editor binds these keys for itself; a binding for the editor
        // inside a console is more specific and takes them.
        KeyBinding::new("secondary-enter", ExecuteStatement, Some(EDITOR)),
        KeyBinding::new("secondary-enter", ExecuteStatement, Some(CONTEXT)),
        KeyBinding::new("ctrl-f2", CancelExecution, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-e", ExplainPlan, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-l", FormatSql, Some(EDITOR)),
        KeyBinding::new("alt-enter", ToggleCodeActions, Some(EDITOR)),
        KeyBinding::new("secondary-b", GoToDefinition, Some(EDITOR)),
        KeyBinding::new("shift-f6", RenameAlias, Some(EDITOR)),
        KeyBinding::new("ctrl-space", ShowCompletions, Some(EDITOR)),
        KeyBinding::new("alt-f7", FindUsages, Some(EDITOR)),
        KeyBinding::new("secondary-p", ParameterInfo, Some(EDITOR)),
        KeyBinding::new("secondary-shift-s", SaveConsoleAs, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("f1", QuickDocumentation, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-q", QuickDocumentation, Some(EDITOR)),
    ]);
}
