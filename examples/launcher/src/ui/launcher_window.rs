use std::{collections::HashMap, rc::Rc, time::Duration};

use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, Keystroke, MouseMoveEvent, ParentElement as _, Pixels,
    Point, Render, ScrollHandle, ScrollStrategy, SharedString, Styled as _, Subscription, Task,
    Window,
    component::{
        ActiveTheme as _, IconName, Sizable as _, VirtualListScrollHandle, WindowExt as _,
        button::{Button, ButtonVariant, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState, MoveLeft, MoveRight},
        progress::Progress,
        select::{Select, SelectEvent, SelectState},
        v_flex,
    },
    div, point,
    prelude::FluentBuilder as _,
    px,
};

use super::{
    Back, CONTEXT, Confirm, ConfirmSecondary, SelectNext, SelectPrevious, ToggleActions,
    action_panel::{OpenPanel, entry_count},
    detail_view::DetailView,
    footer::{Footer, PrimaryHint},
    form_view::{ChoiceItem, FormFields, choice_index, choice_items},
    keyed_id,
    list_view::{Frame, notice},
    toast::show_toast,
};
use crate::{
    extensions::{Catalog, ExtensionHost, LaunchRequest, Opened},
    model::{
        Action, ActionPanel, Confirmation, Dropdown, Effect, ItemId, ListModel, PageModel, Toast,
        ToastStyle,
    },
    pages::{self, PageHandle, RootSearchPage},
    session::{Entry, EntryId, Navigator, Rows},
};

/// How long a page may be loading before the window says so. Shorter waits
/// finish before an indicator could be read, and would only flicker.
const LOADING_DELAY: Duration = Duration::from_millis(150);

/// How close to the end of a list the selection comes before the page is
/// asked for more items.
const LOAD_MORE_THRESHOLD: usize = 5;

/// The launcher's only window: a search field, the current page, a footer.
///
/// The window owns everything the user interacts with: the search field,
/// the selection, the keyboard, overlays and the effects of actions. Pages
/// only supply models, which is why an extension cannot behave differently
/// from a built-in command.
pub struct LauncherWindow {
    focus_handle: FocusHandle,
    input: Entity<InputState>,
    pub(super) navigator: Navigator,
    catalog: Rc<Catalog>,
    extensions: Rc<ExtensionHost>,
    pub(super) list_scroll: VirtualListScrollHandle,
    pub(super) detail_scroll: ScrollHandle,
    /// What the list draws this frame; see [`Frame`].
    pub(super) frame: Rc<Frame>,
    pub(super) action_panel: Option<OpenPanel>,
    /// Retained form fields, by the stack entry of their page.
    pub(super) forms: HashMap<EntryId, FormFields>,
    /// Retained list dropdowns, by the stack entry of their page.
    dropdowns: HashMap<EntryId, ListDropdown>,
    loading: LoadingIndicator,
    /// Where the pointer last moved, so a move to the same place (a list
    /// scrolling under a still pointer) is not taken for a hover.
    pointer: Option<Point<Pixels>>,
    /// The item whose detail the side pane shows, to scroll it back to the
    /// top when the selection moves on.
    detail_item: Option<ItemId>,
    /// Set while the launcher itself writes the search field, so that write is
    /// not mistaken for typing.
    syncing_input: bool,
    /// The placeholder last written to the search field. Writing notifies, so
    /// `render` writes only when it changed.
    applied_placeholder: SharedString,
    _subscriptions: Vec<Subscription>,
}

/// A list's filter dropdown and the value last applied to it from the model.
struct ListDropdown {
    select: Entity<SelectState<Vec<ChoiceItem>>>,
    value: Option<SharedString>,
    _subscription: Subscription,
}

/// Delays the loading indicator by [`LOADING_DELAY`].
#[derive(Default)]
struct LoadingIndicator {
    since: Option<std::time::Instant>,
    _redraw: Option<Task<()>>,
}

impl LauncherWindow {
    pub fn new(
        catalog: Rc<Catalog>,
        extensions: Rc<ExtensionHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let root = cx.new(|_| RootSearchPage::new(&catalog));
        // An extension that updates a command's subtitle is heard in the root
        // search, which lists the command.
        let weak_root = root.downgrade();
        extensions.set_metadata_handler(
            move |command, update, cx| {
                weak_root
                    .update(cx, |root, cx| {
                        root.set_command_subtitle(&command, update.subtitle().cloned(), cx)
                    })
                    .ok();
            },
            cx,
        );
        let root_entry = Self::entry(pages::handle(root), cx);
        let this = cx.entity().downgrade();
        let own_window = window.window_handle();
        let subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input_event),
            cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle() != own_window {
                    return;
                }
                let performed = this
                    .update(cx, |launcher, cx| {
                        launcher.perform_shortcut(&event.keystroke, window, cx)
                    })
                    .unwrap_or(false);
                if performed {
                    cx.stop_propagation();
                }
            }),
        ];

        // Extensions request effects through `launcher/api` while their code
        // runs; they are carried out once that call has returned.
        let handle = window.window_handle();
        let launcher = cx.entity().downgrade();
        extensions.set_effect_handler(move |effect, cx| {
            let launcher = launcher.clone();
            cx.defer(move |cx| {
                handle
                    .update(cx, |_, window, cx| {
                        launcher
                            .update(cx, |launcher, cx| {
                                launcher.perform_effect(effect, window, cx)
                            })
                            .ok();
                    })
                    .ok();
            })
        });

        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            focus_handle: cx.focus_handle(),
            input,
            navigator: Navigator::new(root_entry),
            catalog,
            extensions,
            list_scroll: VirtualListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            frame: Rc::default(),
            action_panel: None,
            forms: HashMap::new(),
            dropdowns: HashMap::new(),
            loading: LoadingIndicator::default(),
            pointer: None,
            detail_item: None,
            syncing_input: false,
            applied_placeholder: SharedString::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Wraps a page for the stack; the window redraws whenever the page does.
    fn entry(page: PageHandle, cx: &mut Context<Self>) -> Entry {
        let launcher = cx.entity().downgrade();
        let subscription = page.observe(
            Box::new(move |cx| {
                launcher.update(cx, |_, cx| cx.notify()).ok();
            }),
            cx,
        );
        Entry::new(page, subscription)
    }

    fn model(&self, window: &mut Window, cx: &mut Context<Self>) -> PageModel {
        self.navigator.current().page().model(window, cx)
    }

    pub(super) fn rows(&self, window: &mut Window, cx: &mut Context<Self>) -> (PageModel, Rows) {
        let model = self.model(window, cx);
        let rows = match &model {
            PageModel::List(list) => Rows::new(list, self.navigator.current().query()),
            PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => {
                Rows::default()
            }
        };
        (model, rows)
    }

    // MARK: Navigation

    /// Pushes a page and gives it a fresh search field.
    fn push(&mut self, page: PageHandle, window: &mut Window, cx: &mut Context<Self>) {
        let entry = Self::entry(page, cx);
        self.navigator.push(entry);
        self.sync_input(window, cx);
        self.page_did_appear(window, cx);
    }

    fn pop(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.navigator.pop() {
            return false;
        }
        self.navigator.current().page().clone().did_reappear(cx);
        self.sync_input(window, cx);
        self.page_did_appear(window, cx);
        true
    }

    fn pop_to_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.pop_to_root();
        self.sync_input(window, cx);
        self.page_did_appear(window, cx);
    }

    /// Settles the window on the page now on top: forgets the state of pages
    /// that left the stack, closes the action panel, scrolls to the top, and
    /// moves focus to where the page's keyboard work happens.
    fn page_did_appear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<EntryId> = self.navigator.ids().collect();
        self.forms.retain(|id, _| ids.contains(id));
        self.dropdowns.retain(|id, _| ids.contains(id));
        self.action_panel = None;
        self.pointer = None;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        self.focus_page(window, cx);
        cx.notify();
    }

    /// Focuses where the current page takes keyboard input: the search field
    /// of a list, the first field of a form, the window itself otherwise.
    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model(window, cx);
        match &model {
            PageModel::List(_) => self.input.update(cx, |input, cx| input.focus(window, cx)),
            PageModel::Form(form) => {
                let entry = self.navigator.current().id();
                self.ensure_form_fields(entry, form, window, cx);
                let focused = self
                    .forms
                    .get(&entry)
                    .is_some_and(|fields| fields.focus_first(form, window, cx));
                if !focused {
                    self.focus_handle.focus(window, cx);
                }
            }
            PageModel::Detail(_) | PageModel::Failure { .. } => self.focus_handle.focus(window, cx),
        }
    }

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) || self.syncing_input {
            return;
        }
        let query = input.read(cx).value();
        self.set_query(query, window, cx);
    }

    /// Records the current page's search text and tells the page. A new query
    /// starts the selection over at the first match.
    fn set_query(&mut self, query: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.navigator.current_mut();
        entry.set_query(query.clone());
        entry.set_selected(None);
        entry.page().clone().set_query(&query, window, cx);
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Writes the search field without reporting it as typing.
    fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.navigator.current().query().clone();
        self.syncing_input = true;
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.syncing_input = false;
    }

    // MARK: Selection

    fn select(&mut self, id: Option<ItemId>, rows: &Rows, cx: &mut Context<Self>) {
        if let Some(line) = id
            .as_ref()
            .and_then(|id| rows.position(id))
            .and_then(|ix| rows.line_of(ix))
        {
            self.list_scroll
                .scroll_to_item(line, ScrollStrategy::Nearest);
        }
        self.navigator.current_mut().set_selected(id);
        cx.notify();
    }

    fn on_select_previous(
        &mut self,
        _: &SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_vertically(-1, window, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(1, window, cx);
    }

    /// Up and down: move the selection a line, or scroll a detail page.
    fn move_vertically(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let (model, rows) = self.rows(window, cx);
        match model {
            PageModel::List(_) => {
                let selected = self.navigator.current().selected().cloned();
                let next = rows
                    .step_lines(selected.as_ref(), delta)
                    .map(|item| item.id().clone());
                if next.is_some() {
                    self.select(next, &rows, cx);
                }
            }
            PageModel::Detail(_) => self.scroll_detail(delta, window, cx),
            PageModel::Form(_) | PageModel::Failure { .. } => cx.propagate(),
        }
    }

    /// Left and right move through a grid. They are taken before the search
    /// field sees them: in a grid, the arrows are navigation, and the caret
    /// is still reachable with Home, End and the word-movement keys.
    fn on_move_horizontally(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(open) = &mut self.action_panel {
            // In the panel, → opens the highlighted submenu and ← leaves one.
            if delta < 0 && open.is_in_submenu() && open.query_is_empty(cx) {
                open.close_submenu(window, cx);
                cx.notify();
                return true;
            }
            if delta > 0
                && let Some(panel) = self.current_panel_actions(window, cx)
                && let Some(open) = &mut self.action_panel
                && let Some(path) = open.highlighted_submenu(&panel, cx)
            {
                open.open_submenu(path, window, cx);
                cx.notify();
                return true;
            }
            return false;
        }
        if !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let (_, rows) = self.rows(window, cx);
        if rows.columns().is_none() {
            return false;
        }
        let selected = self.navigator.current().selected().cloned();
        let next = rows
            .step(selected.as_ref(), delta)
            .map(|item| item.id().clone());
        if next.is_some() {
            self.select(next, &rows, cx);
        }
        true
    }

    fn scroll_detail(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let step = window.rem_size() * 3.;
        let offset = self.detail_scroll.offset();
        let max = self.detail_scroll.max_offset().y;
        let y = (offset.y - step * delta as f32).clamp(-max, px(0.));
        self.detail_scroll.set_offset(point(offset.x, y));
        cx.notify();
    }

    /// A pointer move over an item selects it, unless the pointer did not
    /// actually move.
    pub(super) fn pointer_moved(
        &mut self,
        id: &ItemId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.pointer.replace(event.position) == Some(event.position) {
            return;
        }
        if self.navigator.current().selected() != Some(id) {
            self.navigator.current_mut().set_selected(Some(id.clone()));
            cx.notify();
        }
    }

    /// A click: selects the item and performs its primary action.
    pub(super) fn activate(&mut self, id: ItemId, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.current_mut().set_selected(Some(id));
        self.confirm(false, window, cx);
    }

    // MARK: Actions

    fn on_confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm(false, window, cx);
    }

    fn on_confirm_secondary(
        &mut self,
        _: &ConfirmSecondary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm(true, window, cx);
    }

    /// `Enter` and `Cmd/Ctrl-Enter`. On a form the primary action is
    /// `Cmd/Ctrl-Enter`, so that `Enter` in a field never submits a form
    /// the user has not finished; plain `Enter` does nothing there.
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model(window, cx);
        let action = match (&model, secondary) {
            (PageModel::Form(form), true) => form.actions().primary(),
            (PageModel::Form(_), false) => None,
            (PageModel::Detail(detail), false) => detail.actions().primary(),
            (PageModel::Detail(detail), true) => detail.actions().secondary(),
            (PageModel::List(_), _) => return self.confirm_item(secondary, window, cx),
            (PageModel::Failure { .. }, _) => None,
        };
        if let Some(action) = action.cloned() {
            self.perform(&action, window, cx);
        }
    }

    fn confirm_item(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (_, rows) = self.rows(window, cx);
        let selected = self.navigator.current().selected().cloned();
        let item = rows
            .selected_index(selected.as_ref())
            .and_then(|ix| rows.item(ix))
            .cloned();
        let action = item.as_ref().and_then(|item| match secondary {
            false => item.primary_action(),
            true => item.secondary_action(),
        });
        if let (Some(item), Some(action)) = (&item, action) {
            self.perform_item_action(item.id().clone(), action.clone(), window, cx);
        }
    }

    /// What the footer's primary hint does when clicked.
    pub(super) fn perform_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_form = matches!(self.model(window, cx), PageModel::Form(_));
        self.confirm(on_form, window, cx);
    }

    /// Performs an item's action and tells the page which item it was.
    fn perform_item_action(
        &mut self,
        item: ItemId,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.navigator.current();
        let (page, query) = (entry.page().clone(), entry.query().clone());
        self.perform(&action, window, cx);
        page.did_perform(&item, &query, cx);
    }

    pub(super) fn perform_panel_action(
        &mut self,
        item: Option<ItemId>,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            Some(item) => self.perform_item_action(item, action, window, cx),
            None => self.perform(&action, window, cx),
        }
    }

    /// The actions `Cmd-K` lists and shortcuts reach: the open panel's item,
    /// else the selected item, else the page's own.
    pub(super) fn current_panel_actions(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ActionPanel> {
        let (model, rows) = self.rows(window, cx);
        self.panel_actions_in(&model, &rows)
    }

    /// [`Self::current_panel_actions`] for a model and rows already at hand.
    fn panel_actions_in(&self, model: &PageModel, rows: &Rows) -> Option<ActionPanel> {
        if let Some(panel) = model.page_actions() {
            return Some(panel.clone());
        }
        let id = match &self.action_panel {
            Some(open) => open.item().cloned(),
            None => rows
                .selected_index(self.navigator.current().selected())
                .and_then(|ix| rows.item(ix))
                .map(|item| item.id().clone()),
        }?;
        rows.position(&id)
            .and_then(|ix| rows.item(ix))
            .map(|item| item.actions().clone())
    }

    fn on_toggle_actions(
        &mut self,
        _: &ToggleActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_action_panel(window, cx);
    }

    pub(super) fn toggle_action_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.action_panel.is_some() {
            self.close_action_panel(window, cx);
            return;
        }
        let Some(panel) = self.current_panel_actions(window, cx) else {
            return;
        };
        if entry_count(&panel) == 0 {
            return;
        }
        let (model, rows) = self.rows(window, cx);
        let item = match model {
            PageModel::List(_) => rows
                .selected_index(self.navigator.current().selected())
                .and_then(|ix| rows.item(ix))
                .map(|item| item.id().clone()),
            PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => None,
        };
        self.action_panel = Some(OpenPanel::new(item, window, cx));
        cx.notify();
    }

    /// Closes the panel and gives the keyboard back to what had it: the
    /// field of a form the user was in, say, rather than its first one.
    pub(super) fn close_action_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.action_panel.take() else {
            return;
        };
        match open
            .return_focus()
            .filter(|handle| self.focus_handle.contains(handle, window))
        {
            Some(handle) => handle.focus(window, cx),
            None => self.focus_page(window, cx),
        }
        cx.notify();
    }

    /// Performs the action whose shortcut was pressed, from the open panel's
    /// object, the selected item, or the page, submenus included. Returns
    /// whether one was.
    ///
    /// Shortcuts come from extensions at run time, so they cannot be key
    /// bindings. They are matched before the bindings instead: an action's
    /// shortcut is more specific than the launcher's `Ctrl-N`/`Ctrl-P`
    /// aliases for moving the selection, which `secondary-n` becomes on
    /// Linux and Windows. The keys the launcher is driven by (arrows, `Enter`,
    /// `Esc`, `Tab`, `Cmd/Ctrl-K`) stay the launcher's.
    fn perform_shortcut(
        &mut self,
        pressed: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.focus_handle.contains_focused(window, cx) || window.has_active_dialog(cx) {
            return false;
        }
        let pressed = pressed.unparse();
        let reserved = [
            "up",
            "down",
            "left",
            "right",
            "enter",
            "secondary-enter",
            "escape",
            "tab",
            "shift-tab",
            "secondary-k",
        ];
        if reserved
            .iter()
            .filter_map(|key| Keystroke::parse(key).ok())
            .any(|key| key.unparse() == pressed)
        {
            return false;
        }
        let Some(panel) = self.current_panel_actions(window, cx) else {
            return false;
        };
        let Some(action) = panel
            .all_actions()
            .find(|action| {
                action
                    .shortcut()
                    .and_then(|shortcut| Keystroke::parse(shortcut).ok())
                    .is_some_and(|shortcut| shortcut.unparse() == pressed)
            })
            .cloned()
        else {
            return false;
        };
        let item = match &self.action_panel {
            Some(open) => open.item().cloned(),
            None => {
                let (model, rows) = self.rows(window, cx);
                match model {
                    PageModel::List(_) => rows
                        .selected_index(self.navigator.current().selected())
                        .and_then(|ix| rows.item(ix))
                        .map(|item| item.id().clone()),
                    PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => None,
                }
            }
        };
        self.close_action_panel(window, cx);
        self.perform_panel_action(item, action, window, cx);
        true
    }

    /// `Esc` peels one layer at a time: the action panel (a submenu first),
    /// focus that wandered from the search field, the search text, the page,
    /// and finally the window.
    fn on_back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.action_panel {
            if !open.close_submenu(window, cx) {
                self.close_action_panel(window, cx);
            }
            cx.notify();
            return;
        }
        if matches!(self.model(window, cx), PageModel::List(_)) {
            if !self.input.focus_handle(cx).is_focused(window) {
                self.input.update(cx, |input, cx| input.focus(window, cx));
                return;
            }
            if !self.navigator.current().query().is_empty() {
                self.set_query(SharedString::default(), window, cx);
                self.sync_input(window, cx);
                return;
            }
        }
        if !self.pop(window, cx) {
            self.close(window, cx);
        }
    }

    pub(super) fn perform(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        self.perform_effect(action.effect().clone(), window, cx);
    }

    // MARK: Effects

    /// Carries out an effect. Pages describe effects; only the window performs
    /// them, which is what keeps every page's behavior consistent.
    pub(crate) fn perform_effect(
        &mut self,
        effect: Effect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match effect {
            Effect::OpenUrl(url) => {
                cx.open_url(&url);
                self.close(window, cx);
            }
            Effect::OpenPath(path) => {
                cx.open_with_system(&path);
                self.close(window, cx);
            }
            Effect::RevealPath(path) => {
                cx.reveal_path(&path);
                self.close(window, cx);
            }
            Effect::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                show_toast(
                    &Toast::new(ToastStyle::Success, "Copied to clipboard"),
                    window,
                    cx,
                );
            }
            Effect::Paste(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                self.close(window, cx);
                crate::shell::platform::paste_into_previous_application(cx);
            }
            Effect::ShowToast(toast) => show_toast(&toast, window, cx),
            Effect::ShowHud(text) => {
                self.close(window, cx);
                crate::shell::platform::show_hud(text, cx);
            }
            Effect::Launch(request) => self.launch(&request, window, cx),
            Effect::Push(build) => match build.build(window, cx) {
                Ok(page) => self.push(page, window, cx),
                Err(error) => show_toast(
                    &Toast::new(ToastStyle::Failure, "Couldn’t open the page")
                        .with_message(format!("{error:#}")),
                    window,
                    cx,
                ),
            },
            Effect::Pop => {
                self.pop(window, cx);
            }
            Effect::PopToRoot => self.pop_to_root(window, cx),
            Effect::CloseWindow => self.close(window, cx),
            Effect::Confirm(confirmation) => self.confirm_effect(confirmation, window, cx),
            Effect::SubmitForm(handler) => {
                let values = match self.model(window, cx) {
                    PageModel::Form(form) => {
                        let entry = self.navigator.current().id();
                        self.ensure_form_fields(entry, &form, window, cx);
                        self.forms
                            .get(&entry)
                            .map(|fields| fields.values(&form, cx))
                            .unwrap_or_default()
                    }
                    PageModel::List(_) | PageModel::Detail(_) | PageModel::Failure { .. } => {
                        Default::default()
                    }
                };
                handler.call(values, window, cx);
            }
            Effect::Run(handler) => handler.run(window, cx),
        }
        cx.notify();
    }

    /// Asks before performing a consequential effect. The confirming button
    /// names the result and is drawn as destructive when the result is.
    fn confirm_effect(
        &mut self,
        confirmation: Confirmation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launcher = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let effect = confirmation.effect().clone();
            let launcher = launcher.clone();
            alert
                .title(confirmation.title().clone())
                .when_some(confirmation.message().cloned(), |this, message| {
                    this.description(message)
                })
                .show_cancel(true)
                .cancel_text("Cancel")
                .ok_text(confirmation.confirm_title().clone())
                .when(confirmation.is_destructive(), |this| {
                    this.ok_variant(ButtonVariant::Danger)
                })
                .on_ok(move |_, window, cx| {
                    let effect = effect.clone();
                    // After the dialog has closed, so the effect sees the
                    // launcher, not the dialog, as the focused surface.
                    let launcher = launcher.clone();
                    window.defer(cx, move |window, cx| {
                        launcher
                            .update(cx, |launcher, cx| {
                                launcher.perform_effect(effect, window, cx)
                            })
                            .ok();
                    });
                    true
                })
        });
    }

    /// Returns to a fresh root search with the search field focused, as the
    /// launcher should look each time it is summoned.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.pop_to_root();
        self.set_query(SharedString::default(), window, cx);
        self.sync_input(window, cx);
        self.page_did_appear(window, cx);
    }

    /// Opens a command from outside the window, such as a deep link.
    pub fn open_command(
        &mut self,
        request: LaunchRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset(window, cx);
        self.launch(&request, window, cx);
    }

    fn launch(&mut self, request: &LaunchRequest, window: &mut Window, cx: &mut Context<Self>) {
        let id = request.command();
        let Some((extension, command)) = self.catalog.command(id) else {
            show_toast(
                &Toast::new(ToastStyle::Failure, "Couldn’t find the command")
                    .with_message(id.to_string()),
                window,
                cx,
            );
            return;
        };
        match self
            .extensions
            .open(extension, command, request, window, cx)
        {
            Ok(Opened::Page(page)) => self.push(page, window, cx),
            Ok(Opened::Background) => {}
            Err(error) => {
                tracing::error!("{error:#}");
                show_toast(
                    &Toast::new(ToastStyle::Failure, "Couldn’t open the command")
                        .with_message(format!("{error:#}")),
                    window,
                    cx,
                );
            }
        }
    }

    /// Returns to a fresh root search and gets out of the way. Hiding goes
    /// through the shell, because only macOS can hide a window in place.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset(window, cx);
        crate::shell::launcher::hide(cx);
    }

    // MARK: Per-frame bookkeeping

    /// Honors a page's requested selection and tells the page about the
    /// selection and the approaching end of its list, once per change. The
    /// page callbacks run after this frame, never inside it.
    fn sync_list_state(
        &mut self,
        list: &ListModel,
        rows: &Rows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.navigator.current_mut();
        if entry.request_selection(list.selected()) {
            let id = entry.selected().cloned();
            self.select(id, rows, cx);
        }
        let entry = self.navigator.current_mut();
        let effective = rows
            .selected_index(entry.selected())
            .and_then(|ix| rows.item(ix))
            .map(|item| item.id().clone());
        if let Some(changed) = entry.take_selection_change(effective.as_ref())
            && let Some(handler) = list.on_selection_change().cloned()
        {
            cx.defer_in(window, move |_, window, cx| {
                handler.call(changed.as_shared().clone(), window, cx)
            });
        }
        if let Some(handler) = list.on_load_more().cloned()
            && entry.should_load_more(
                rows.items_after(entry.selected()),
                rows.item_count(),
                LOAD_MORE_THRESHOLD,
            )
        {
            cx.defer_in(window, move |_, window, cx| handler.run(window, cx));
        }
        if effective != self.detail_item {
            self.detail_item = effective;
            self.detail_scroll.set_offset(point(px(0.), px(0.)));
        }
    }

    /// Whether to show that the page is loading: only once it has been for
    /// [`LOADING_DELAY`], so quick work never flashes an indicator.
    fn loading_visible(&mut self, loading: bool, cx: &mut Context<Self>) -> bool {
        let now = cx.background_executor().now();
        match (loading, self.loading.since) {
            (false, _) => {
                self.loading = LoadingIndicator::default();
                false
            }
            (true, Some(since)) => now.duration_since(since) >= LOADING_DELAY,
            (true, None) => {
                let timer = cx.background_executor().timer(LOADING_DELAY);
                self.loading = LoadingIndicator {
                    since: Some(now),
                    _redraw: Some(cx.spawn(async move |this, cx| {
                        timer.await;
                        this.update(cx, |_, cx| cx.notify()).ok();
                    })),
                };
                false
            }
        }
    }

    /// Keeps the list's dropdown in step with the model, creating it the
    /// first time the page shows one.
    fn sync_dropdown(&mut self, dropdown: &Dropdown, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.navigator.current().id();
        match self.dropdowns.get_mut(&entry) {
            Some(existing) => {
                if existing.value.as_ref() != dropdown.value() {
                    existing.value = dropdown.value().cloned();
                    let index = choice_index(dropdown.choices(), dropdown.value());
                    existing.select.update(cx, |select, cx| {
                        select.set_items(choice_items(dropdown.choices()), window, cx);
                        select.set_selected_index(index, window, cx);
                    });
                }
            }
            None => {
                let index = choice_index(dropdown.choices(), dropdown.value());
                let items = choice_items(dropdown.choices());
                let select = cx.new(|cx| SelectState::new(items, index, window, cx));
                let subscription = cx.subscribe_in(
                    &select,
                    window,
                    move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        this.dropdown_changed(entry, value.clone(), window, cx);
                    },
                );
                self.dropdowns.insert(
                    entry,
                    ListDropdown {
                        select,
                        value: dropdown.value().cloned(),
                        _subscription: subscription,
                    },
                );
            }
        }
    }

    fn dropdown_changed(
        &mut self,
        entry: EntryId,
        value: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigator.current().id() != entry {
            return;
        }
        if let Some(dropdown) = self.dropdowns.get_mut(&entry) {
            dropdown.value = Some(value.clone());
        }
        if let PageModel::List(list) = self.model(window, cx)
            && let Some(handler) = list.dropdown().and_then(Dropdown::on_change)
        {
            handler.call(value, window, cx);
        }
        // The choice is made; typing goes back to the search field.
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    // MARK: Drawing

    fn render_search_bar(
        &mut self,
        model: &PageModel,
        show_loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let can_go_back = self.navigator.depth() > 1;
        let title = self.navigator.current().page().title(cx);
        let dropdown = match model {
            PageModel::List(list) => list.dropdown().and_then(|dropdown| {
                self.dropdowns
                    .get(&self.navigator.current().id())
                    .map(|state| (state.select.clone(), dropdown.tooltip().clone()))
            }),
            _ => None,
        };
        let theme = cx.theme();
        h_flex()
            .relative()
            .flex_none()
            .h_12()
            .gap_2()
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .map(|this| match can_go_back {
                true => this.child(
                    Button::new("back")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronLeft)
                        .accessibility_label("Back")
                        // Esc is the keyboard path back; the button is for the pointer.
                        .tab_stop(false)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.pop(window, cx);
                        })),
                ),
                false => this.child(
                    gpui_kit::component::Icon::new(IconName::Search)
                        .text_color(theme.muted_foreground),
                ),
            })
            .map(|this| match model {
                // A list is searched, so its page shows the search field.
                PageModel::List(_) => {
                    this.child(Input::new(&self.input).appearance(false).p_0().flex_1())
                }
                // A detail or form page has nothing to search. A disabled
                // field would suggest otherwise, so the page's title takes
                // its place, naming what the page is about.
                PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => this
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(title),
                    ),
            })
            .when_some(dropdown, |this, (select, tooltip)| {
                this.child(
                    Select::new(&select)
                        .small()
                        .w_48()
                        .accessibility_label(tooltip),
                )
            })
            .when(show_loading, |this| {
                this.child(
                    div().absolute().left_0().right_0().bottom_0().child(
                        Progress::new("page-loading")
                            .loading(true)
                            .with_size(gpui_kit::component::Size::Size(
                                // A hairline-weight bar that rides the
                                // search bar's bottom edge.
                                window.rem_size() * 0.125,
                            ))
                            .rounded_none()
                            .accessibility_label("Loading"),
                    ),
                )
            })
    }
}

impl Render for LauncherWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (model, rows) = self.rows(window, cx);
        let entry = self.navigator.current().id();
        match &model {
            PageModel::List(list) => {
                self.sync_list_state(list, &rows, window, cx);
                if let Some(dropdown) = list.dropdown() {
                    self.sync_dropdown(dropdown, window, cx);
                }
            }
            PageModel::Form(form) => self.ensure_form_fields(entry, form, window, cx),
            PageModel::Detail(_) | PageModel::Failure { .. } => {}
        }
        let selected_ix = rows.selected_index(self.navigator.current().selected());
        self.frame = Rc::new(Frame::new(rows, selected_ix, window));

        let placeholder: SharedString = match &model {
            PageModel::List(list) => list.placeholder().cloned().unwrap_or("Search…".into()),
            PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => {
                SharedString::default()
            }
        };
        if self.applied_placeholder != placeholder {
            self.applied_placeholder = placeholder.clone();
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            });
        }

        // The panel closes when the object it was opened for is gone.
        if let Some(open) = &self.action_panel
            && let Some(item) = open.item()
            && self.frame.rows().position(item).is_none()
        {
            self.action_panel = None;
            cx.defer_in(window, |this, window, cx| this.focus_page(window, cx));
        }

        // Keep the keyboard from getting lost: if focus left the window's
        // content while no dialog holds it (a control that disappeared with
        // its page, say), put it back where the page takes input.
        if !self.focus_handle.contains_focused(window, cx) && !window.has_active_dialog(cx) {
            let launcher = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                launcher
                    .update(cx, |launcher, cx| {
                        if !launcher.focus_handle.contains_focused(window, cx)
                            && !window.has_active_dialog(cx)
                        {
                            launcher.focus_page(window, cx);
                        }
                    })
                    .ok();
            });
        }

        let show_loading = self.loading_visible(model.is_loading(), cx);
        let panel_actions = self.panel_actions_in(&model, self.frame.rows());
        let primary = match &model {
            PageModel::Form(form) => form
                .actions()
                .primary()
                .map(|action| PrimaryHint::new(action.title().clone(), "secondary-enter")),
            PageModel::Detail(detail) => detail
                .actions()
                .primary()
                .map(|action| PrimaryHint::new(action.title().clone(), "enter")),
            PageModel::List(_) => self
                .frame
                .selected_item()
                .and_then(|item| item.primary_action())
                .map(|action| PrimaryHint::new(action.title().clone(), "enter")),
            PageModel::Failure { .. } => None,
        };
        let title = self.navigator.current().page().title(cx);
        let launcher = cx.entity().downgrade();

        let body = match &model {
            PageModel::List(list) => self.render_list(list, cx),
            PageModel::Detail(detail) => DetailView::new(
                keyed_id("page-detail", entry_key(entry)),
                detail.clone(),
                &self.detail_scroll,
                launcher.clone(),
            )
            .into_any_element(),
            PageModel::Form(form) => self.render_form(entry, form, cx),
            PageModel::Failure { title, message } => {
                notice(title.clone(), Some(message.clone()), cx)
            }
        };
        let search_bar = self.render_search_bar(&model, show_loading, window, cx);

        v_flex()
            .id("launcher")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_select_previous))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_confirm_secondary))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_toggle_actions))
            .capture_action(cx.listener(|this, _: &MoveLeft, window, cx| {
                if this.on_move_horizontally(-1, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveRight, window, cx| {
                if this.on_move_horizontally(1, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(search_bar)
            .child(div().relative().flex_1().min_h_0().child(body).when_some(
                self.action_panel.as_ref().zip(panel_actions.as_ref()),
                |this, (open, actions)| {
                    this.child(div().absolute().right_2().bottom_2().child(open.render(
                        actions,
                        launcher.clone(),
                        cx,
                    )))
                },
            ))
            .child(
                Footer::new(title, launcher)
                    .loading(show_loading)
                    .primary(primary)
                    .shows_actions(panel_actions.is_some_and(|panel| entry_count(&panel) > 1)),
            )
    }
}

/// An entry id as an element id key, so each detail page parses its own body.
fn entry_key(entry: EntryId) -> SharedString {
    format!("{entry:?}").into()
}

#[cfg(test)]
mod extension_flows;

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, path::PathBuf, rc::Rc};

    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::Context as GpuiContext;

    use super::*;
    use crate::{
        model::{
            ActionEntry, ActionSection, ActionStyle, Callback, Choice, Control, DetailModel, Field,
            FormModel, FormValue, FormValues, Item, Layout, Metadata, MetadataValue, RunHandler,
            Section, Submenu, Tag, TextHandler, Tone,
        },
        pages::Page,
        session::Row,
    };

    pub(super) fn bundled() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions")
    }

    /// Keeps a test's data directory alive, and removes it, with the app.
    struct TestData {
        _directory: tempfile::TempDir,
    }

    impl gpui_kit::Global for TestData {}

    /// Opens the launcher on the extensions under `roots`, with a data
    /// directory and a secret store of its own.
    pub(super) fn open(
        cx: &mut TestAppContext,
        roots: &[PathBuf],
    ) -> (Entity<LauncherWindow>, VisualTestContext) {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().to_path_buf();
        cx.update(|cx| cx.set_global(TestData { _directory: data }));
        open_in(
            cx,
            roots,
            &path,
            Rc::new(crate::extensions::MemorySecrets::default()),
        )
    }

    /// Opens the launcher with its data in `data` and its passwords in
    /// `secrets`. Tests never touch the user's data directory or keychain.
    pub(super) fn open_in(
        cx: &mut TestAppContext,
        roots: &[PathBuf],
        data: &std::path::Path,
        secrets: Rc<crate::extensions::MemorySecrets>,
    ) -> (Entity<LauncherWindow>, VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_shell::init(cx);
            crate::ui::init(cx);
        });
        let catalog = Rc::new(Catalog::discover(roots));
        let extensions = Rc::new(
            cx.update(|cx| {
                ExtensionHost::new_in(crate::extensions::DataDirectory::new(data), secrets, cx)
            })
            .unwrap(),
        );
        let (handle, launcher) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, move |window, cx| {
                    cx.new(|cx| LauncherWindow::new(catalog, extensions, window, cx))
                })
            })
            .unwrap();
        let cx = VisualTestContext::from_window(handle, cx);
        cx.run_until_parked();
        (launcher, cx)
    }

    /// The visible rows as `# Header` or `item-id`, with `>` on the selection
    /// and the item's accessory after a colon.
    pub(super) fn rows(
        launcher: &Entity<LauncherWindow>,
        cx: &mut VisualTestContext,
    ) -> Vec<String> {
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                let (_, rows) = launcher.rows(window, cx);
                let selected = rows.selected_index(launcher.navigator.current().selected());
                rows.rows()
                    .iter()
                    .enumerate()
                    .map(|(ix, row)| match row {
                        Row::Header { title, .. } => format!("# {title}"),
                        Row::Item(item) => format!(
                            "{}{}{}",
                            if selected == Some(ix) { ">" } else { "" },
                            item.id().as_str(),
                            item.accessories()
                                .first()
                                .and_then(|a| a.label())
                                .map(|a| format!(":{a}"))
                                .unwrap_or_default()
                        ),
                    })
                    .collect()
            })
        })
    }

    pub(super) fn selected(
        launcher: &Entity<LauncherWindow>,
        cx: &mut VisualTestContext,
    ) -> String {
        rows(launcher, cx)
            .into_iter()
            .find_map(|row| row.strip_prefix('>').map(str::to_owned))
            .unwrap_or_default()
    }

    pub(super) fn page(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> PageModel {
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.rows(window, cx).0))
    }

    pub(super) fn depth(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| launcher.read(cx).navigator.depth())
    }

    pub(super) fn panel_is_open(
        launcher: &Entity<LauncherWindow>,
        cx: &mut VisualTestContext,
    ) -> bool {
        cx.update(|_, cx| launcher.read(cx).action_panel.is_some())
    }

    /// A page whose model the test decides, and which records what the
    /// window tells it.
    struct TestPage {
        build: Rc<dyn Fn() -> PageModel>,
    }

    impl Page for TestPage {
        fn title(&self) -> SharedString {
            "Test".into()
        }

        fn model(&mut self, _: &mut Window, _: &mut GpuiContext<Self>) -> PageModel {
            (self.build)()
        }

        fn set_query(&mut self, _: &str, _: &mut Window, cx: &mut GpuiContext<Self>) {
            cx.notify();
        }
    }

    /// Pushes a page built by `build` on top of the root search.
    fn push_page(
        launcher: &Entity<LauncherWindow>,
        cx: &mut VisualTestContext,
        build: impl Fn() -> PageModel + 'static,
    ) -> Entity<TestPage> {
        let build: Rc<dyn Fn() -> PageModel> = Rc::new(build);
        let page = cx.update(|window, cx| {
            let page = cx.new(|_| TestPage { build });
            launcher.update(cx, |launcher, cx| {
                launcher.push(pages::handle(page.clone()), window, cx)
            });
            page
        });
        cx.run_until_parked();
        page
    }

    type Log = Rc<RefCell<Vec<String>>>;

    fn log_run(log: &Log, entry: &str) -> Effect {
        let (log, entry) = (log.clone(), entry.to_owned());
        Effect::Run(RunHandler::new(move |(), _, _| {
            log.borrow_mut().push(entry.clone())
        }))
    }

    fn item(id: &str) -> Item {
        Item::new(ItemId::new(id), id)
    }

    #[gpui::test]
    fn test_root_search_lists_extension_commands_without_running_them(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[bundled()]);
        assert_eq!(
            rows(&launcher, &mut cx),
            [
                "# Extensions",
                ">com.gpui-kit.emoji/search-emoji:Command",
                "com.gpui-kit.github/search-repositories:Command",
                "com.gpui-kit.links/links:Command",
                "com.gpui-kit.links/checklist:Command",
                "com.gpui-kit.links/search-docs:Command",
                "com.gpui-kit.links/copy-date:Command",
                "com.gpui-kit.notes/search-notes:Command",
                "com.gpui-kit.notes/create-note:Command",
                "# System",
                "system/toggle-appearance:Command",
                "system/settings:Command",
                "system/extensions:Command",
                "system/quit:Command",
            ]
        );

        cx.simulate_input("check");
        assert_eq!(
            rows(&launcher, &mut cx)[..2],
            ["# Results", ">com.gpui-kit.links/checklist:Command"]
        );
    }

    #[gpui::test]
    fn test_an_extension_page_is_drawn_and_driven_by_the_launcher(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[bundled()]);
        cx.simulate_input("checklist");
        cx.simulate_keystrokes("enter");
        assert_eq!(depth(&launcher, &mut cx), 2, "Enter opens the command");
        assert_eq!(
            rows(&launcher, &mut cx),
            [">install:Done", "run", "guides", "extension"]
        );
        let PageModel::List(list) = page(&launcher, &mut cx) else {
            panic!("the checklist renders a List");
        };
        assert_eq!(
            list.placeholder().map(|p| p.as_ref()),
            Some("Filter tasks (3 remaining)…"),
            "the script sees its launch context and its own state"
        );

        // `run` calls back into the script; its `cx.notify()` rebuilds the
        // page, and the selection stays on the item that changed.
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            rows(&launcher, &mut cx),
            ["install:Done", ">run:Done", "guides", "extension"]
        );
        cx.simulate_keystrokes("enter");
        assert_eq!(
            rows(&launcher, &mut cx),
            ["install:Done", ">run", "guides", "extension"],
            "the rebuilt page carries fresh callbacks"
        );

        // The launcher filters an extension's list like its own.
        cx.simulate_input("guides");
        assert_eq!(rows(&launcher, &mut cx), [">guides"]);

        // Esc clears the search, then returns to the root search as it was.
        cx.simulate_keystrokes("escape");
        assert_eq!(rows(&launcher, &mut cx).len(), 4);
        cx.simulate_keystrokes("escape");
        assert_eq!(depth(&launcher, &mut cx), 1);
        assert_eq!(
            rows(&launcher, &mut cx)[1],
            ">com.gpui-kit.links/checklist:Command",
            "the root search keeps its query and selection"
        );
    }

    #[gpui::test]
    fn test_actions_the_launcher_performs_itself(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[bundled()]);
        cx.simulate_input("links");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            rows(&launcher, &mut cx),
            [
                "# Documentation",
                ">home",
                "design",
                "coding",
                "# Community",
                "github",
                "issues"
            ]
        );

        cx.simulate_keystrokes("down secondary-shift-c");
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("https://gpui-kit.com/docs/design-guides"),
            "an action's shortcut performs it without running extension code"
        );
        cx.simulate_keystrokes("secondary-enter");
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("https://gpui-kit.com/docs/design-guides"),
            "the second action is the secondary action"
        );
    }

    #[gpui::test]
    fn test_a_command_that_returns_no_list_explains_why(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("launcher-broken-{}", std::process::id()));
        let extension = root.join("broken");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("gpui-shell.json"),
            r#"{ "id": "test.broken", "name": "Broken", "entry": "main.js" }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("launcher.json"),
            r#"{ "commands": [{ "name": "plain", "title": "Plain", "module": "main.js" }] }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("main.js"),
            r#"import { View, div } from "gpui-kit";
export default class Plain extends View { render() { return div().child("hello"); } }"#,
        )
        .unwrap();

        let (launcher, mut cx) = open(cx, std::slice::from_ref(&root));
        cx.simulate_input("plain");
        cx.simulate_keystrokes("enter");
        let model = page(&launcher, &mut cx);
        std::fs::remove_dir_all(&root).ok();
        let PageModel::Failure { message, .. } = model else {
            panic!("a page that is not a List is reported, not drawn");
        };
        assert!(message.contains("must return a `List`"), "{message}");
    }

    #[gpui::test]
    fn test_grid_arrows_move_in_two_dimensions(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        push_page(&launcher, &mut cx, || {
            PageModel::List(
                ListModel::new()
                    .with_layout(Layout::Grid { columns: 3 })
                    .with_section(
                        Section::new()
                            .with_title("Smileys")
                            .with_items(["a", "b", "c", "d", "e"].map(item)),
                    )
                    .with_section(
                        Section::new()
                            .with_title("Animals")
                            .with_items(["f", "g"].map(item)),
                    ),
            )
        });
        assert_eq!(selected(&launcher, &mut cx), "a");
        cx.simulate_keystrokes("right");
        assert_eq!(selected(&launcher, &mut cx), "b");
        cx.simulate_keystrokes("down");
        assert_eq!(selected(&launcher, &mut cx), "e", "down keeps the column");
        cx.simulate_keystrokes("down");
        assert_eq!(selected(&launcher, &mut cx), "g", "and crosses sections");
        cx.simulate_keystrokes("left left");
        assert_eq!(
            selected(&launcher, &mut cx),
            "e",
            "left wraps to the line above"
        );
        cx.simulate_keystrokes("up");
        assert_eq!(selected(&launcher, &mut cx), "b");
    }

    #[gpui::test]
    fn test_action_panel_filters_opens_submenus_and_performs(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let log: Log = Rc::default();
        let actions = {
            let log = log.clone();
            move || {
                ActionPanel::new()
                    .with_action(Action::new("Open", log_run(&log, "open")))
                    .with_action(Action::new("Copy Link", log_run(&log, "copy")))
                    .with_submenu(
                        Submenu::new("Set Priority")
                            .with_action(Action::new("High", log_run(&log, "high")))
                            .with_action(Action::new("Low", log_run(&log, "low"))),
                    )
                    .with_section(
                        ActionSection::new()
                            .with_title("Danger")
                            .with_entry(ActionEntry::Action(
                                Action::new("Delete", log_run(&log, "delete"))
                                    .with_style(ActionStyle::Destructive)
                                    .with_shortcut("ctrl-shift-d"),
                            )),
                    )
            }
        };
        push_page(&launcher, &mut cx, move || {
            PageModel::List(ListModel::new().with_item(item("task").with_actions(actions())))
        });

        cx.simulate_keystrokes("secondary-k");
        assert!(panel_is_open(&launcher, &mut cx));

        // The panel's own search filters its actions; Enter performs the
        // highlighted one, and performing closes the panel.
        cx.simulate_input("copy");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["copy"]);
        assert!(!panel_is_open(&launcher, &mut cx));

        // A submenu opens with → and leaves with ←, then Esc closes.
        cx.simulate_keystrokes("secondary-k down down right");
        cx.run_until_parked();
        assert!(cx.update(|_, cx| {
            launcher
                .read(cx)
                .action_panel
                .as_ref()
                .is_some_and(OpenPanel::is_in_submenu)
        }));
        cx.simulate_keystrokes("left");
        assert!(cx.update(|_, cx| {
            launcher
                .read(cx)
                .action_panel
                .as_ref()
                .is_some_and(|open| !open.is_in_submenu())
        }));
        cx.simulate_keystrokes("escape");
        assert!(!panel_is_open(&launcher, &mut cx));
        assert_eq!(depth(&launcher, &mut cx), 2, "Esc closed only the panel");

        // Enter opens a submenu, Esc returns from it before closing the panel.
        cx.simulate_keystrokes("secondary-k down down enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["copy", "low"]);
        assert!(!panel_is_open(&launcher, &mut cx));

        // Shortcuts work without the panel, submenus and sections included.
        cx.simulate_keystrokes("ctrl-shift-d");
        assert_eq!(*log.borrow(), ["copy", "low", "delete"]);
    }

    #[gpui::test]
    fn test_form_fields_keep_input_tab_through_and_submit(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let submitted: Rc<RefCell<Option<FormValues>>> = Rc::default();
        let changes: Log = Rc::default();
        {
            let submitted = submitted.clone();
            let changes = changes.clone();
            push_page(&launcher, &mut cx, move || {
                let submitted = submitted.clone();
                let changes = changes.clone();
                PageModel::Form(
                    FormModel::new()
                        .with_field(
                            Field::new(
                                "title",
                                "Title",
                                Control::Text {
                                    placeholder: None,
                                    value: "".into(),
                                },
                            )
                            .with_on_change(Callback::new(
                                move |value, _, _| {
                                    if let FormValue::Text(text) = value {
                                        changes.borrow_mut().push(text.to_string());
                                    }
                                },
                            )),
                        )
                        .with_field(Field::new(
                            "notes",
                            "Notes",
                            Control::TextArea {
                                placeholder: None,
                                value: "".into(),
                            },
                        ))
                        .with_field(Field::new(
                            "urgent",
                            "Urgent",
                            Control::Checkbox {
                                label: "Mark as urgent".into(),
                                value: true,
                            },
                        ))
                        .with_field(Field::new(
                            "list",
                            "List",
                            Control::Dropdown {
                                choices: vec![
                                    Choice::new("home", "Home"),
                                    Choice::new("work", "Work"),
                                ],
                                value: Some("work".into()),
                            },
                        ))
                        .with_field(Field::new(
                            "due",
                            "Due",
                            Control::Date {
                                value: Some("2026-09-26".into()),
                            },
                        ))
                        .with_actions(ActionPanel::new().with_action(Action::new(
                            "Create Task",
                            Effect::SubmitForm(Callback::new(move |values, _, _| {
                                submitted.replace(Some(values));
                            })),
                        ))),
                )
            });
        }

        cx.simulate_input("Buy");
        assert_eq!(changes.borrow().last().map(String::as_str), Some("Buy"));
        cx.simulate_keystrokes("tab");
        cx.simulate_input("milk");
        // Tab leaves even the multi-line field, which would otherwise indent.
        cx.simulate_keystrokes("tab shift-tab shift-tab");
        cx.simulate_input("!");

        // Plain Enter never submits; Cmd/Ctrl-Enter does.
        cx.simulate_keystrokes("enter");
        assert!(submitted.borrow().is_none());
        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();
        let values = submitted.borrow().clone().expect("the form was submitted");
        assert_eq!(values.get("title"), Some(&FormValue::Text("Buy!".into())));
        assert_eq!(values.get("notes"), Some(&FormValue::Text("milk".into())));
        assert_eq!(values.get("urgent"), Some(&FormValue::Bool(true)));
        assert_eq!(values.get("list"), Some(&FormValue::Text("work".into())));
        assert_eq!(
            values.get("due"),
            Some(&FormValue::Text("2026-09-26".into()))
        );
    }

    #[gpui::test]
    fn test_a_detail_page_scrolls_and_performs_its_own_actions(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let log: Log = Rc::default();
        {
            let log = log.clone();
            push_page(&launcher, &mut cx, move || {
                PageModel::Detail(
                    DetailModel::new(format!("# Release\n\n{}", "Line of notes.\n\n".repeat(80)))
                        .with_metadata(Metadata::new(
                            "Status",
                            MetadataValue::Tags(vec![Tag::new("Shipped").with_tone(Tone::Success)]),
                        ))
                        .with_metadata(Metadata::new("", MetadataValue::Separator))
                        .with_metadata(Metadata::new(
                            "Website",
                            MetadataValue::Link {
                                text: "gpui-kit.com".into(),
                                url: "https://gpui-kit.com".into(),
                            },
                        ))
                        .with_actions(
                            ActionPanel::new()
                                .with_action(Action::new("Publish", log_run(&log, "publish")))
                                .with_action(Action::new("Archive", log_run(&log, "archive"))),
                        ),
                )
            });
        }
        let offset = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| launcher.read(cx).detail_scroll.offset().y)
        };
        assert_eq!(offset(&mut cx), px(0.));
        cx.simulate_keystrokes("down down");
        assert!(offset(&mut cx) < px(0.), "down scrolls the body");
        cx.simulate_keystrokes("up up");
        assert_eq!(offset(&mut cx), px(0.));

        cx.simulate_keystrokes("enter secondary-enter");
        assert_eq!(*log.borrow(), ["publish", "archive"]);

        // The page's own actions are in the panel too.
        cx.simulate_keystrokes("secondary-k");
        assert!(panel_is_open(&launcher, &mut cx));
        cx.simulate_keystrokes("escape escape");
        assert_eq!(
            depth(&launcher, &mut cx),
            1,
            "Esc closes the panel, then pops"
        );
    }

    #[gpui::test]
    fn test_confirmation_performs_only_when_confirmed(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let log: Log = Rc::default();
        let effect = Effect::Confirm(
            Confirmation::new("Delete “Roadmap”?", log_run(&log, "deleted"))
                .with_confirm_title("Delete")
                .destructive(true),
        );

        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.perform_effect(effect.clone(), window, cx)
            })
        });
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(log.borrow().is_empty(), "cancel performs nothing");
        assert_eq!(
            depth(&launcher, &mut cx),
            1,
            "and Esc did not reach the page"
        );

        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.perform_effect(effect, window, cx)
            })
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["deleted"]);
        assert!(
            cx.update(|window, cx| launcher.read(cx).focus_handle.contains_focused(window, cx)),
            "focus returns to the launcher"
        );
    }

    #[gpui::test]
    fn test_detail_pane_follows_the_selection(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        push_page(&launcher, &mut cx, || {
            PageModel::List(
                ListModel::new()
                    .with_showing_detail(true)
                    .with_item(item("a").with_detail(DetailModel::new("# Alpha")))
                    .with_item(item("b").with_detail(DetailModel::new("# Beta"))),
            )
        });
        let shown = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                launcher
                    .read(cx)
                    .frame
                    .selected_item()
                    .and_then(|item| item.detail())
                    .map(|detail| detail.markdown().to_string())
            })
        };
        assert_eq!(shown(&mut cx).as_deref(), Some("# Alpha"));
        cx.simulate_keystrokes("down");
        assert_eq!(shown(&mut cx).as_deref(), Some("# Beta"));
    }

    #[gpui::test]
    fn test_selection_and_load_more_are_reported_once(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let selections: Log = Rc::default();
        let loads: Log = Rc::default();
        let count = Rc::new(std::cell::Cell::new(8));
        let page = {
            let (selections, loads, count) = (selections.clone(), loads.clone(), count.clone());
            push_page(&launcher, &mut cx, move || {
                let selections = selections.clone();
                let loads = loads.clone();
                PageModel::List(
                    ListModel::new()
                        .with_section(
                            Section::new()
                                .with_items((0..count.get()).map(|ix| item(&ix.to_string()))),
                        )
                        .with_on_selection_change(TextHandler::new(move |id, _, _| {
                            selections.borrow_mut().push(id.to_string())
                        }))
                        .with_on_load_more(RunHandler::new(move |(), _, _| {
                            loads.borrow_mut().push("more".into())
                        })),
                )
            })
        };
        assert_eq!(
            *selections.borrow(),
            ["0"],
            "the first selection is reported"
        );
        cx.simulate_keystrokes("down");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(
            *selections.borrow(),
            ["0", "1"],
            "once per change, not per frame"
        );

        assert!(loads.borrow().is_empty());
        cx.simulate_keystrokes("down down");
        cx.run_until_parked();
        assert_eq!(loads.borrow().len(), 1, "within five items of the end");
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(loads.borrow().len(), 1, "once until the list grows");

        count.set(16);
        cx.update(|_, cx| page.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.simulate_keystrokes("down down down down down down");
        cx.run_until_parked();
        assert_eq!(loads.borrow().len(), 1, "eleven to go is not near the end");
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(loads.borrow().len(), 2, "re-armed by the longer list");
    }

    #[gpui::test]
    fn test_a_toast_with_an_id_replaces_the_last_one(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        let toast =
            |style, title: &str, id: &str| Effect::ShowToast(Toast::new(style, title).with_id(id));
        for effect in [
            toast(ToastStyle::Progress, "Uploading…", "upload"),
            toast(ToastStyle::Success, "Uploaded", "upload"),
            toast(ToastStyle::Info, "Other", "other"),
        ] {
            cx.update(|window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.perform_effect(effect, window, cx)
                })
            });
        }
        cx.run_until_parked();
        assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 2);
    }

    #[gpui::test]
    fn test_escape_peels_layers_in_order(cx: &mut TestAppContext) {
        let (launcher, mut cx) = open(cx, &[]);
        push_page(&launcher, &mut cx, || {
            PageModel::List(
                ListModel::new().with_item(
                    item("a")
                        .with_action(Action::new("Open", Effect::Pop))
                        .with_action(Action::new("Copy", Effect::Copy("a".into()))),
                ),
            )
        });
        cx.simulate_input("a");
        cx.simulate_keystrokes("secondary-k");
        assert!(panel_is_open(&launcher, &mut cx));
        cx.simulate_keystrokes("escape");
        assert!(!panel_is_open(&launcher, &mut cx));
        assert_eq!(
            cx.update(|window, cx| launcher.read(cx).input.focus_handle(cx).is_focused(window)),
            true,
            "closing the panel gives the keyboard back to the search field"
        );
        cx.simulate_keystrokes("escape");
        assert_eq!(depth(&launcher, &mut cx), 2, "the query went first");
        cx.simulate_keystrokes("escape");
        assert_eq!(depth(&launcher, &mut cx), 1, "then the page");
    }
}
