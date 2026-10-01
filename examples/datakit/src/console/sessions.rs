//! Every open console, for the Services window.

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global, WeakEntity};

use super::ConsolePanel;

#[derive(Clone, Debug)]
pub enum SessionsEvent {
    /// A console opened or closed.
    Changed,
}

/// The consoles of the application, weakly: a closed console drops out.
#[derive(Default)]
pub struct Sessions {
    consoles: Vec<WeakEntity<ConsolePanel>>,
}

impl EventEmitter<SessionsEvent> for Sessions {}

struct GlobalSessions(Entity<Sessions>);

impl Global for GlobalSessions {}

impl Sessions {
    pub fn init(cx: &mut App) {
        let sessions = cx.new(|_| Sessions::default());
        cx.set_global(GlobalSessions(sessions));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSessions>().0.clone()
    }

    pub(super) fn register(console: &Entity<ConsolePanel>, cx: &mut App) {
        let weak = console.downgrade();
        Self::global(cx).update(cx, |sessions, cx| {
            sessions
                .consoles
                .retain(|console| console.upgrade().is_some());
            sessions.consoles.push(weak);
            cx.emit(SessionsEvent::Changed);
        });
        // Closing the console takes it off the list.
        let sessions = Self::global(cx);
        cx.observe_release(console, move |_, cx| {
            sessions.update(cx, |sessions, cx| {
                sessions
                    .consoles
                    .retain(|console| console.upgrade().is_some());
                cx.emit(SessionsEvent::Changed);
            });
        })
        .detach();
    }

    /// The consoles still open, in the order they opened.
    pub fn consoles(&self) -> Vec<Entity<ConsolePanel>> {
        self.consoles
            .iter()
            .filter_map(|console| console.upgrade())
            .collect()
    }
}
