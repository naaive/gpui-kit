use gpui_kit::{SharedString, Subscription};

use crate::{model::ItemId, pages::PageHandle};

/// Identifies an entry on the stack for as long as it is there.
///
/// Window state that belongs to one page — form fields, a dropdown — is
/// keyed by it, so pushing the same page twice gives each its own state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EntryId(u64);

/// One page on the stack, with the search text and selection it had.
///
/// Both are kept per page, so returning from a command restores the root
/// search exactly as the user left it.
pub struct Entry {
    id: EntryId,
    page: PageHandle,
    query: SharedString,
    selected: Option<ItemId>,
    /// The last selection the page asked for, so a request is honored once
    /// and the user's own selection wins until the page asks for another.
    requested: Option<ItemId>,
    /// The last selection reported to the page's `on_selection_change`.
    reported: Option<ItemId>,
    /// The item count when `on_load_more` last fired; it fires again only
    /// once the list has grown past it.
    loaded_more_at: Option<usize>,
    _subscription: Subscription,
}

impl Entry {
    pub fn new(page: PageHandle, subscription: Subscription) -> Self {
        Self {
            id: EntryId(0),
            page,
            query: SharedString::default(),
            selected: None,
            requested: None,
            reported: None,
            loaded_more_at: None,
            _subscription: subscription,
        }
    }

    pub fn id(&self) -> EntryId {
        self.id
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

    /// Adopts the selection a page asks for when it asks for a different one
    /// than last time. Returns whether the selection changed.
    pub fn request_selection(&mut self, requested: Option<&ItemId>) -> bool {
        if requested == self.requested.as_ref() {
            return false;
        }
        self.requested = requested.cloned();
        match requested {
            Some(id) if self.selected.as_ref() != Some(id) => {
                self.selected = Some(id.clone());
                true
            }
            _ => false,
        }
    }

    /// Records the item actually selected and returns it when it differs
    /// from the one last reported, so a page hears about each change once.
    pub fn take_selection_change(&mut self, effective: Option<&ItemId>) -> Option<ItemId> {
        if effective == self.reported.as_ref() {
            return None;
        }
        self.reported = effective.cloned();
        effective.cloned()
    }

    /// Whether to ask the page for more items: the selection is within
    /// `threshold` items of the end and the list has grown since the last
    /// time it asked.
    pub fn should_load_more(
        &mut self,
        items_after: usize,
        item_count: usize,
        threshold: usize,
    ) -> bool {
        let near_end = item_count > 0 && items_after < threshold;
        let grown = self.loaded_more_at.is_none_or(|count| item_count > count);
        if near_end && grown {
            self.loaded_more_at = Some(item_count);
            true
        } else {
            false
        }
    }
}

/// The page stack. The root page is at the bottom and is never popped.
pub struct Navigator {
    stack: Vec<Entry>,
    next_id: u64,
}

impl Navigator {
    pub fn new(root: Entry) -> Self {
        let mut navigator = Self {
            stack: Vec::new(),
            next_id: 0,
        };
        navigator.push(root);
        navigator
    }

    pub fn current(&self) -> &Entry {
        self.stack.last().expect("the root page is never popped")
    }

    pub fn current_mut(&mut self) -> &mut Entry {
        self.stack
            .last_mut()
            .expect("the root page is never popped")
    }

    /// Every entry on the stack, bottom first.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.stack.iter()
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The ids of every entry on the stack, bottom first.
    pub fn ids(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.stack.iter().map(Entry::id)
    }

    pub fn push(&mut self, mut entry: Entry) {
        self.next_id += 1;
        entry.id = EntryId(self.next_id);
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

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use gpui_kit::{AppContext as _, Context, Window};

    use super::*;
    use crate::{
        model::{ListModel, PageModel},
        pages::{self, Page},
    };

    struct Blank;

    impl Page for Blank {
        fn title(&self) -> SharedString {
            "Blank".into()
        }

        fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
            PageModel::List(ListModel::new())
        }

        fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
    }

    fn entry(cx: &mut TestAppContext) -> Entry {
        cx.update(|cx| {
            let page = cx.new(|_| Blank);
            let subscription = cx.observe(&page, |_, _| {});
            Entry::new(pages::handle(page) as PageHandle, subscription)
        })
    }

    #[gpui::test]
    fn test_entries_get_distinct_ids(cx: &mut TestAppContext) {
        let mut navigator = Navigator::new(entry(cx));
        navigator.push(entry(cx));
        let ids: Vec<_> = navigator.ids().collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        navigator.pop();
        navigator.push(entry(cx));
        assert_ne!(navigator.current().id(), ids[1], "ids are never reused");
    }

    #[gpui::test]
    fn test_selection_bookkeeping(cx: &mut TestAppContext) {
        let mut entry = entry(cx);
        let a = ItemId::new("a");
        let b = ItemId::new("b");

        // A request is adopted once; the user's choice then wins.
        assert!(entry.request_selection(Some(&b)));
        entry.set_selected(Some(a.clone()));
        assert!(!entry.request_selection(Some(&b)));
        assert_eq!(entry.selected(), Some(&a));

        // Each change is reported once.
        assert_eq!(entry.take_selection_change(Some(&a)), Some(a.clone()));
        assert_eq!(entry.take_selection_change(Some(&a)), None);
        assert_eq!(entry.take_selection_change(Some(&b)), Some(b.clone()));

        // Loading more fires near the end, once per growth of the list.
        assert!(!entry.should_load_more(10, 20, 5));
        assert!(entry.should_load_more(4, 20, 5));
        assert!(!entry.should_load_more(2, 20, 5));
        assert!(entry.should_load_more(3, 40, 5));
    }
}
