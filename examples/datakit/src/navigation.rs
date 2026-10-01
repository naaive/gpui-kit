//! Requests to show something, from wherever they start — a console's Go to
//! Declaration, a diagram, Search Everywhere — to the workspace that owns
//! the window's layout.

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};

use crate::objects::ObjectRef;

#[derive(Clone)]
pub enum NavigationEvent {
    /// Select the object in the database explorer.
    Reveal(ObjectRef),
    /// Open the object: a relation's data, another object's source.
    Open(ObjectRef),
    /// Open a console for the object's data source holding `sql`.
    OpenConsole { object: ObjectRef, sql: String },
}

/// The application's navigation requests. Subscribe to receive them.
pub struct Navigation;

impl EventEmitter<NavigationEvent> for Navigation {}

struct GlobalNavigation(Entity<Navigation>);

impl Global for GlobalNavigation {}

impl Navigation {
    pub fn init(cx: &mut App) {
        let navigation = cx.new(|_| Navigation);
        cx.set_global(GlobalNavigation(navigation));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalNavigation>().0.clone()
    }

    pub fn request(event: NavigationEvent, cx: &mut App) {
        Self::global(cx).update(cx, |_, cx| cx.emit(event));
    }
}
