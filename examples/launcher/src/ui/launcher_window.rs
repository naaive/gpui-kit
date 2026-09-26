use std::rc::Rc;

use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    KeyDownEvent, Keystroke, ParentElement as _, Render, ScrollHandle, SharedString, Styled as _,
    Subscription, Window,
    component::{
        ActiveTheme as _, Icon, IconName, WindowExt as _, h_flex,
        input::{Input, InputEvent, InputState},
        notification::{Notification, NotificationType},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{
    Back, CONTEXT, Confirm, ConfirmSecondary, SelectNext, SelectPrevious, footer::Footer,
    list_view::ListView,
};
use crate::{
    extensions::Opened,
    extensions::{Catalog, ExtensionHost, LaunchRequest},
    model::{Action, Effect, FormValues, ItemId, PageModel, Toast, ToastStyle},
    pages::{self, PageHandle, RootSearchPage},
    session::{Entry, Navigator, Rows},
};

/// The launcher's only window: a search field, the current page, a footer.
///
/// The window owns everything the user interacts with: the search field,
/// the selection, the keyboard and the effects of actions. Pages only supply
/// models, which is why an extension cannot behave differently from a
/// built-in command.
pub struct LauncherWindow {
    input: Entity<InputState>,
    navigator: Navigator,
    catalog: Rc<Catalog>,
    extensions: Rc<ExtensionHost>,
    scroll: ScrollHandle,
    /// Set while the launcher itself writes the search field, so that write is
    /// not mistaken for typing.
    syncing_input: bool,
    /// The placeholder last written to the search field. Writing notifies, so
    /// `render` writes only when it changed.
    applied_placeholder: SharedString,
    _subscriptions: Vec<Subscription>,
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
        let root_entry = Self::entry(pages::handle(root), cx);
        let subscriptions = vec![cx.subscribe_in(&input, window, Self::on_input_event)];

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
            input,
            navigator: Navigator::new(root_entry),
            catalog,
            extensions,
            scroll: ScrollHandle::new(),
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

    /// Pushes a page and gives it a fresh search field.
    fn push(&mut self, page: PageHandle, window: &mut Window, cx: &mut Context<Self>) {
        let entry = Self::entry(page, cx);
        self.navigator.push(entry);
        self.sync_input(window, cx);
        self.scroll.scroll_to_item(0);
        cx.notify();
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
        self.scroll.scroll_to_item(0);
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

    fn rows(&self, window: &mut Window, cx: &mut Context<Self>) -> (PageModel, Rows) {
        let entry = self.navigator.current();
        let model = entry.page().model(window, cx);
        let rows = match &model {
            PageModel::List(list) => Rows::new(list, entry.query()),
            PageModel::Detail(_) | PageModel::Form(_) | PageModel::Failure { .. } => {
                Rows::default()
            }
        };
        (model, rows)
    }

    fn select(&mut self, id: Option<ItemId>, rows: &Rows, cx: &mut Context<Self>) {
        if let Some(ix) = rows.selected_index(id.as_ref()) {
            self.scroll.scroll_to_item(ix);
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
        self.step(-1, window, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let (_, rows) = self.rows(window, cx);
        let selected = self.navigator.current().selected().cloned();
        let next = rows
            .step(selected.as_ref(), delta)
            .map(|item| item.id().clone());
        if next.is_some() {
            self.select(next, &rows, cx);
        }
    }

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

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Performs the selected item's action whose shortcut was pressed.
    ///
    /// Shortcuts come from extensions at run time, so they cannot be key
    /// bindings; they are matched here instead, after the bindings above had
    /// their chance.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pressed = event.keystroke.unparse();
        let (_, rows) = self.rows(window, cx);
        let selected = self.navigator.current().selected().cloned();
        let matches = |action: &&Action| {
            action
                .shortcut()
                .and_then(|shortcut| Keystroke::parse(shortcut).ok())
                .is_some_and(|shortcut| shortcut.unparse() == pressed)
        };
        let item = rows
            .selected_index(selected.as_ref())
            .and_then(|ix| rows.item(ix))
            .cloned();
        if let Some(item) = item {
            if let Some(action) = item.actions().all_actions().find(matches).cloned() {
                cx.stop_propagation();
                self.perform_item_action(item.id().clone(), action, window, cx);
            }
        } else if let Some(action) = self
            .rows(window, cx)
            .0
            .page_actions()
            .and_then(|panel| panel.all_actions().find(matches).cloned())
        {
            cx.stop_propagation();
            self.perform(&action, window, cx);
        }
    }

    /// A click: selects the item and performs its primary action.
    pub(super) fn activate(&mut self, id: ItemId, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.current_mut().set_selected(Some(id));
        self.confirm(false, window, cx);
    }

    /// `Esc` peels one layer at a time: the search text, then the page.
    fn on_back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigator.current().query().is_empty() {
            self.set_query(SharedString::default(), window, cx);
            self.sync_input(window, cx);
            return;
        }
        if self.navigator.pop() {
            self.sync_input(window, cx);
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    pub(super) fn perform(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        self.perform_effect(action.effect().clone(), window, cx);
    }

    /// Carries out an effect. Pages describe effects; only the window performs
    /// them, which is what keeps every page's behavior consistent.
    pub(crate) fn perform_effect(
        &mut self,
        effect: Effect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match effect {
            Effect::OpenUrl(url) => cx.open_url(&url),
            Effect::OpenPath(path) => cx.open_with_system(&path),
            Effect::RevealPath(path) => cx.reveal_path(&path),
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
                    &Toast::new(ToastStyle::Failure, "Cannot open the page")
                        .with_message(format!("{error:#}")),
                    window,
                    cx,
                ),
            },
            Effect::Pop => {
                if self.navigator.pop() {
                    self.sync_input(window, cx);
                }
            }
            Effect::PopToRoot => {
                self.navigator.pop_to_root();
                self.sync_input(window, cx);
            }
            Effect::CloseWindow => self.close(window, cx),
            Effect::Confirm(confirmation) => {
                self.perform_effect(confirmation.effect().clone(), window, cx)
            }
            Effect::SubmitForm(handler) => handler.call(FormValues::new(), window, cx),
            Effect::Run(handler) => handler.run(window, cx),
        }
        cx.notify();
    }

    /// Returns to a fresh root search with the search field focused, as the
    /// launcher should look each time it is summoned.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.pop_to_root();
        self.set_query(SharedString::default(), window, cx);
        self.sync_input(window, cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
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
                &Toast::new(ToastStyle::Failure, format!("No command `{id}`")),
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
                    &Toast::new(ToastStyle::Failure, "Cannot open the command")
                        .with_message(format!("{error:#}")),
                    window,
                    cx,
                );
            }
        }
    }

    /// Returns to a fresh root search and gets out of the way.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator.pop_to_root();
        self.set_query(SharedString::default(), window, cx);
        self.sync_input(window, cx);
        cx.hide();
    }
}

fn show_toast(toast: &Toast, window: &mut Window, cx: &mut gpui_kit::App) {
    let kind = match toast.style() {
        ToastStyle::Info | ToastStyle::Progress => NotificationType::Info,
        ToastStyle::Success => NotificationType::Success,
        ToastStyle::Failure => NotificationType::Error,
    };
    let notification = Notification::new()
        .title(toast.title().clone())
        .with_type(kind);
    let notification = match toast.message() {
        Some(message) => notification.message(message.clone()),
        None => notification,
    };
    window.push_notification(notification, cx);
}

impl Render for LauncherWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (model, rows) = self.rows(window, cx);
        let selected_ix = rows.selected_index(self.navigator.current().selected());
        let selected = selected_ix.and_then(|ix| rows.item(ix)).cloned();
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
        let can_go_back = self.navigator.depth() > 1;
        let loading = model.is_loading();
        let title = self.navigator.current().page().title(cx);

        v_flex()
            .id("launcher")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_select_previous))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_confirm_secondary))
            .on_action(cx.listener(Self::on_back))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when(can_go_back, |this| {
                        this.child(
                            Icon::new(IconName::ChevronLeft)
                                .text_color(cx.theme().muted_foreground),
                        )
                    })
                    .when(!can_go_back, |this| {
                        this.child(
                            Icon::new(IconName::Search).text_color(cx.theme().muted_foreground),
                        )
                    })
                    .child(Input::new(&self.input).appearance(false).p_0().flex_1()),
            )
            .child(div().flex_1().min_h_0().child(ListView::new(
                &model,
                &rows,
                selected_ix,
                &self.scroll,
                cx.entity(),
            )))
            .child(Footer::new(title, selected, loading))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, ops::Deref as _, path::PathBuf, rc::Rc};

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::session::Row;

    fn bundled() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions")
    }

    fn open(
        cx: &mut TestAppContext,
        roots: &[PathBuf],
    ) -> (Entity<LauncherWindow>, VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_shell::init(cx);
            crate::ui::init(cx);
        });
        let catalog = Rc::new(Catalog::discover(roots));
        let extensions = Rc::new(cx.update(ExtensionHost::new).unwrap());
        let slot = Rc::new(RefCell::new(None));
        let launcher = slot.clone();
        let window = cx.add_window(move |window, cx| {
            let view = cx.new(|cx| LauncherWindow::new(catalog, extensions, window, cx));
            launcher.replace(Some(view.clone()));
            gpui_kit::base::Root::new(view, window, cx)
        });
        let cx = VisualTestContext::from_window(*window.deref(), cx);
        cx.run_until_parked();
        let launcher = slot.borrow().clone().unwrap();
        (launcher, cx)
    }

    /// The visible rows as `# Header` or `item-id`, with `>` on the selection
    /// and the item's accessory after a colon.
    fn rows(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                let (_, rows) = launcher.rows(window, cx);
                let selected = rows.selected_index(launcher.navigator.current().selected());
                rows.rows()
                    .iter()
                    .enumerate()
                    .map(|(ix, row)| match row {
                        Row::Header(title) => format!("# {title}"),
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

    fn page(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> PageModel {
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.rows(window, cx).0))
    }

    fn depth(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| launcher.read(cx).navigator.depth())
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
                "# Launcher",
                "system/website:Link",
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
}
