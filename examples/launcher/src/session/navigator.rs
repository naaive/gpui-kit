use gpui_kit::{SharedString, Subscription};

use crate::{model::ItemId, pages::PageHandle};

/// One page on the stack, with the search text and selection it had.
///
/// Both are kept per page, so returning from a command restores the root
/// search exactly as the user left it.
pub struct Entry {
    page: PageHandle,
    query: SharedString,
    selected: Option<ItemId>,
    _subscription: Subscription,
}

impl Entry {
    pub fn new(page: PageHandle, subscription: Subscription) -> Self {
        Self {
            page,
            query: SharedString::default(),
            selected: None,
            _subscription: subscription,
        }
    }

    pub fn page(&self) -> &PageHandle {
        &self.page
    }

    pub fn query(&self) -> &SharedString {
        &self.query
    }

    pub fn set_query(&mut self, query: SharedString) {
        self.query = query;
    }

    pub fn selected(&self) -> Option<&ItemId> {
        self.selected.as_ref()
    }

    pub fn set_selected(&mut self, selected: Option<ItemId>) {
        self.selected = selected;
    }
}

/// The page stack. The root page is at the bottom and is never popped.
pub struct Navigator {
    stack: Vec<Entry>,
}

impl Navigator {
    pub fn new(root: Entry) -> Self {
        Self { stack: vec![root] }
    }

    pub fn current(&self) -> &Entry {
        self.stack.last().expect("the root page is never popped")
    }

    pub fn current_mut(&mut self) -> &mut Entry {
        self.stack
            .last_mut()
            .expect("the root page is never popped")
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn push(&mut self, entry: Entry) {
        self.stack.push(entry);
    }

    /// Returns whether a page was popped; the root page is not.
    pub fn pop(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    pub fn pop_to_root(&mut self) {
        self.stack.truncate(1);
    }
}
