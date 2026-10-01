//! The Files tool window: folders of SQL scripts attached to DataKit, opened
//! into consoles that save back to the file.

mod scan;

use std::{collections::HashMap, path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::Button,
    dock::{BasePanel, Panel, PanelControl, PanelEvent},
    h_flex,
    list::ListItem,
    tree::{TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Task, Window, actions, div,
    prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use crate::services::Services;

use scan::{FileEntry, scan};

actions!(
    files,
    [
        /// Open the selected SQL file in a console.
        OpenSelectedFile
    ]
);

const CONTEXT: &str = "Files";

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("f4", OpenSelectedFile, Some(CONTEXT))]);
}

#[derive(Clone, Debug)]
pub enum FilesEvent {
    /// Open the SQL file in a console.
    Open(PathBuf),
}

/// What a row of the tree stands for.
#[derive(Clone)]
enum Node {
    /// An attached folder, which can be detached.
    Root(PathBuf),
    Folder,
    File(PathBuf),
}

pub struct FilesPanel {
    focus_handle: FocusHandle,
    tree: Entity<TreeState>,
    folders: Vec<PathBuf>,
    nodes: Rc<HashMap<SharedString, Node>>,
    expanded: std::collections::HashSet<SharedString>,
    scan_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for FilesPanel {}
impl EventEmitter<FilesEvent> for FilesPanel {}

impl FilesPanel {
    pub const NAME: &str = "Files";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, window, cx| {
                this.tree.update(cx, |tree, cx| tree.focus(window, cx));
            }),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.clone());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id);
                }
            }),
        ];
        let folders = std::fs::read(folders_path(cx))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        let mut panel = Self {
            focus_handle,
            tree,
            folders,
            nodes: Rc::default(),
            expanded: Default::default(),
            scan_task: None,
            _subscriptions: subscriptions,
        };
        panel.rescan(cx);
        panel
    }

    /// Ask for a folder and add its SQL files to the tree.
    pub fn attach_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                for path in paths {
                    if !this.folders.contains(&path) {
                        this.expanded.insert(root_id(&path));
                        this.folders.push(path);
                    }
                }
                this.save(cx);
                this.rescan(cx);
            });
        })
        .detach();
    }

    fn detach_folder(&mut self, folder: &PathBuf, cx: &mut Context<Self>) {
        self.folders.retain(|attached| attached != folder);
        self.save(cx);
        self.rescan(cx);
    }

    fn save(&self, cx: &App) {
        let path = folders_path(cx);
        let json = serde_json::to_vec_pretty(&self.folders).unwrap_or_default();
        cx.background_spawn(async move {
            if let Some(directory) = path.parent() {
                let _ = std::fs::create_dir_all(directory);
            }
            if let Err(error) = std::fs::write(&path, json) {
                tracing::error!("couldn’t save the attached folders: {error}");
            }
        })
        .detach();
    }

    /// Read the attached folders again.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        let folders = self.folders.clone();
        self.scan_task = Some(cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn(async move {
                    folders
                        .iter()
                        .map(|folder| scan(folder))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| this.show(entries, cx));
        }));
    }

    fn show(&mut self, entries: Vec<FileEntry>, cx: &mut Context<Self>) {
        let mut nodes = HashMap::new();
        let items: Vec<TreeItem> = entries
            .iter()
            .map(|entry| {
                let id = root_id(entry.path());
                nodes.insert(id.clone(), Node::Root(entry.path().to_path_buf()));
                let children = self.items(entry, &id, &mut nodes);
                TreeItem::new(id.clone(), entry.name())
                    .expanded(self.expanded.contains(&id))
                    .children(children)
            })
            .collect();
        self.nodes = Rc::new(nodes);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        cx.notify();
    }

    fn items(
        &self,
        folder: &FileEntry,
        parent: &SharedString,
        nodes: &mut HashMap<SharedString, Node>,
    ) -> Vec<TreeItem> {
        folder
            .children()
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                let id: SharedString = format!("{parent}/{}", entry.name()).into();
                match entry.children() {
                    Some(_) => {
                        nodes.insert(id.clone(), Node::Folder);
                        let children = self.items(entry, &id, nodes);
                        TreeItem::new(id.clone(), entry.name())
                            .expanded(self.expanded.contains(&id))
                            .children(children)
                    }
                    None => {
                        nodes.insert(id.clone(), Node::File(entry.path().to_path_buf()));
                        TreeItem::new(id, entry.name())
                    }
                }
            })
            .collect()
    }

    fn open_selected(&mut self, _: &OpenSelectedFile, _: &mut Window, cx: &mut Context<Self>) {
        let node = self
            .tree
            .read(cx)
            .selected_item()
            .and_then(|item| self.nodes.get(&item.id))
            .cloned();
        if let Some(Node::File(path)) = node {
            cx.emit(FilesEvent::Open(path));
        }
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let nodes = self.nodes.clone();
        let menu_nodes = self.nodes.clone();
        let panel = cx.entity().downgrade();
        let menu_panel = panel.clone();
        tree(&self.tree, move |ix, entry, selected, _, cx| {
            let item = entry.item();
            let node = nodes.get(&item.id).cloned();
            let muted = cx.theme().muted_foreground;
            let icon = match &node {
                Some(Node::File(_)) => IconName::FileCode,
                _ if entry.is_expanded() => IconName::FolderOpen,
                _ => IconName::Folder,
            };
            let disclosure = div().flex_none().size_3().when(entry.is_folder(), |slot| {
                slot.child(
                    Icon::new(if entry.is_expanded() {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .text_color(muted),
                )
            });
            let panel = panel.clone();
            ListItem::new(ix)
                .selected(selected)
                .py_0p5()
                .pl(rems(0.25 + entry.depth() as f32 * 1.0))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1p5()
                        .text_sm()
                        .child(disclosure)
                        .child(Icon::new(icon).xsmall().text_color(muted))
                        .child(div().min_w_0().truncate().child(item.label.clone())),
                )
                .on_click(move |event, _, cx| {
                    if event.click_count() == 2
                        && let Some(Node::File(path)) = &node
                    {
                        let path = path.clone();
                        let _ = panel.update(cx, |_, cx| cx.emit(FilesEvent::Open(path)));
                    }
                })
        })
        .context_menu(move |_, entry, menu, _, _| {
            let panel = menu_panel.clone();
            match menu_nodes.get(&entry.item().id).cloned() {
                Some(Node::File(_)) => menu.menu_with_icon(
                    t!("files.open").to_string(),
                    IconName::SquareTerminal,
                    Box::new(OpenSelectedFile),
                ),
                Some(Node::Root(folder)) => menu.item(
                    gpui_kit::component::menu::PopupMenuItem::new(t!("files.detach").to_string())
                        .on_click(move |_, _, cx| {
                            let _ = panel.update(cx, |panel, cx| panel.detach_folder(&folder, cx));
                        }),
                ),
                _ => menu,
            }
        })
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .text_sm()
            .child(
                div()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("files.empty").to_string()),
            )
            .child(
                Button::new("attach-folder")
                    .small()
                    .label(t!("files.attach").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.attach_folder(cx))),
            )
    }
}

fn root_id(path: &std::path::Path) -> SharedString {
    format!("root:{}", path.display()).into()
}

fn folders_path(cx: &App) -> PathBuf {
    Services::global(cx).data_directory().join("folders.json")
}

impl Focusable for FilesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for FilesPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for FilesPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("files.title").to_string()
    }

    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let attach = cx.entity().downgrade();
        let refresh = cx.entity().downgrade();
        Some(vec![
            Button::new("files-attach")
                .icon(IconName::Plus)
                .tooltip(t!("files.attach").to_string())
                .on_click(move |_, _, cx| {
                    let _ = attach.update(cx, |panel, cx| panel.attach_folder(cx));
                }),
            Button::new("files-refresh")
                .icon(IconName::RefreshCw)
                .tooltip(t!("files.refresh").to_string())
                .on_click(move |_, _, cx| {
                    let _ = refresh.update(cx, |panel, cx| panel.rescan(cx));
                }),
        ])
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_selected))
            .map(|panel| {
                if self.folders.is_empty() {
                    panel.child(self.render_empty(cx))
                } else {
                    panel.child(div().flex_1().min_h_0().p_1().child(self.render_tree(cx)))
                }
            })
    }
}
