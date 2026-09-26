use std::{fmt, rc::Rc};

use anyhow::Result;
use gpui_kit::{App, SharedString, Window};

use super::FormValues;
use crate::pages::PageHandle;

/// Behavior owned by the page that produced a model.
///
/// Built-in pages pass Rust closures; extension pages pass closures that call
/// back into their script. Either way the window only ever sees a `Callback`.
pub struct Callback<A>(Rc<dyn Fn(A, &mut Window, &mut App)>);

impl<A> Callback<A> {
    pub fn new(handler: impl Fn(A, &mut Window, &mut App) + 'static) -> Self {
        Self(Rc::new(handler))
    }

    pub fn call(&self, argument: A, window: &mut Window, cx: &mut App) {
        (self.0)(argument, window, cx)
    }
}

impl<A> Clone for Callback<A> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A> fmt::Debug for Callback<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Callback")
    }
}

/// Runs something: `Action.run`, loading the next page of a list.
pub type RunHandler = Callback<()>;
/// Receives text: the search query, a dropdown value, a selected item id.
pub type TextHandler = Callback<SharedString>;
/// Receives the values of a submitted form.
pub type FormHandler = Callback<FormValues>;

impl RunHandler {
    pub fn run(&self, window: &mut Window, cx: &mut App) {
        self.call((), window, cx)
    }
}

/// Builds the page an action pushes.
#[derive(Clone)]
pub struct PushHandler(Rc<dyn Fn(&mut Window, &mut App) -> Result<PageHandle>>);

impl PushHandler {
    pub fn new(build: impl Fn(&mut Window, &mut App) -> Result<PageHandle> + 'static) -> Self {
        Self(Rc::new(build))
    }

    pub fn build(&self, window: &mut Window, cx: &mut App) -> Result<PageHandle> {
        (self.0)(window, cx)
    }
}

impl fmt::Debug for PushHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PushHandler")
    }
}
