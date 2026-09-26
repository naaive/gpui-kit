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

use gpui_kit::{App, Context, Entity, SharedString, Window};

use crate::model::PageModel;

pub trait Page: 'static + Sized {
    /// Shown in the footer while this page is on top.
    fn title(&self) -> SharedString;

    fn model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> PageModel;

    /// Called whenever the search text changes while this page is on top.
    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>);
}

/// A page with its type erased, as the navigation stack holds it.
pub trait AnyPage {
    fn title(&self, cx: &App) -> SharedString;
    fn model(&self, window: &mut Window, cx: &mut App) -> PageModel;
    fn set_query(&self, query: &str, window: &mut Window, cx: &mut App);
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
}

pub type PageHandle = Rc<dyn AnyPage>;
