//! The pages that can sit on the navigation stack.
//!
//! A page produces a [`PageModel`] and optionally reacts to the search text.
//! Built-in pages and extension pages implement the same trait, which is what
//! lets one renderer draw both.

mod root_search;
mod script_page;

pub use root_search::RootSearchPage;
pub use script_page::ScriptPage;

use std::rc::Rc;

use gpui_kit::{App, Context, Entity, SharedString, Subscription, Window};

use crate::model::{ItemId, PageModel};

pub trait Page: 'static + Sized {
    /// Shown in the footer while this page is on top.
    fn title(&self) -> SharedString;

    fn model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> PageModel;

    /// Called whenever the search text changes while this page is on top.
    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>);

    /// Called after one of this page's item actions was performed, so a page
    /// can learn from what the user picks (the root search ranks by it).
    fn did_perform(&mut self, _item: &ItemId, _query: &str, _cx: &mut Context<Self>) {}

    /// Called when the page is on top again after the page above it was
    /// popped. What that page did may have changed what this one shows.
    fn did_reappear(&mut self, _cx: &mut Context<Self>) {}
}

/// A page with its type erased, as the navigation stack holds it.
pub trait AnyPage {
    fn title(&self, cx: &App) -> SharedString;
    fn model(&self, window: &mut Window, cx: &mut App) -> PageModel;
    fn set_query(&self, query: &str, window: &mut Window, cx: &mut App);
    fn did_perform(&self, item: &ItemId, query: &str, cx: &mut App);
    fn did_reappear(&self, cx: &mut App);
    /// Calls `on_notify` whenever the page asks to be drawn again.
    fn observe(&self, on_notify: Box<dyn Fn(&mut App)>, cx: &mut App) -> Subscription;
}

impl<P: Page> AnyPage for Entity<P> {
    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).title()
    }

    fn model(&self, window: &mut Window, cx: &mut App) -> PageModel {
        self.update(cx, |page, cx| page.model(window, cx))
    }

    fn set_query(&self, query: &str, window: &mut Window, cx: &mut App) {
        self.update(cx, |page, cx| page.set_query(query, window, cx))
    }

    fn did_perform(&self, item: &ItemId, query: &str, cx: &mut App) {
        self.update(cx, |page, cx| page.did_perform(item, query, cx))
    }

    fn did_reappear(&self, cx: &mut App) {
        self.update(cx, |page, cx| page.did_reappear(cx))
    }

    fn observe(&self, on_notify: Box<dyn Fn(&mut App)>, cx: &mut App) -> Subscription {
        cx.observe(self, move |_, cx| on_notify(cx))
    }
}

pub type PageHandle = Rc<dyn AnyPage>;

/// Erases a page entity into the handle the navigation stack holds.
pub fn handle<P: Page>(page: Entity<P>) -> PageHandle {
    Rc::new(page)
}
