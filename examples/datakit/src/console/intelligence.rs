//! What the console's editor knows about SQL beyond completion: quick
//! documentation on hover, Go to Declaration, and inspections with their
//! quick fixes.

use std::{cell::RefCell, ops::Range, rc::Rc, str::FromStr as _};

use anyhow::Result;
use datakit_sql::{Inspection, Problem, Target, resolve};
use gpui_kit::component::input::{
    CodeActionProvider, DefinitionProvider, EditorState, HoverProvider, Rope, RopeExt as _,
};
use gpui_kit::{App, Entity, SharedString, Task, WeakEntity, Window};
use lsp_types::{CodeAction, CodeActionKind, LocationLink, TextEdit, Uri, WorkspaceEdit};
use rust_i18n::t;

use crate::{
    datasource::DataSource,
    objects::{ObjectPath, ObjectRef},
};

/// The address of the console's own text in Go to Declaration locations.
const DOCUMENT: &str = "datakit://console";
/// The prefix of a catalog object's address; the rest indexes
/// [`SqlIntelligence::targets`].
const OBJECT: &str = "datakit://object/";

/// Hover, definitions and quick fixes for one console.
pub struct SqlIntelligence {
    data_source: WeakEntity<DataSource>,
    /// The latest inspections of the console's text, which quick fixes read.
    inspections: Rc<RefCell<Vec<Inspection>>>,
    /// Objects Go to Declaration found, by the index in their address.
    targets: Rc<RefCell<Vec<ObjectPath>>>,
}

impl SqlIntelligence {
    pub fn new(
        data_source: WeakEntity<DataSource>,
        inspections: Rc<RefCell<Vec<Inspection>>>,
    ) -> Self {
        Self {
            data_source,
            inspections,
            targets: Rc::default(),
        }
    }

    /// The object behind a Go to Declaration address, if it is one.
    pub fn object_for(&self, uri: &Uri) -> Option<ObjectRef> {
        let index: usize = uri.as_str().strip_prefix(OBJECT)?.parse().ok()?;
        let path = self.targets.borrow().get(index)?.clone();
        Some(ObjectRef::new(self.data_source.upgrade()?, path))
    }

    fn resolution(&self, text: &Rope, offset: usize, cx: &App) -> Option<(Range<usize>, Target)> {
        let data_source = self.data_source.upgrade()?;
        let source = data_source.read(cx);
        let text = text.to_string();
        let resolution = resolve(&text, offset, source.catalog(), &*source.dialect())?;
        Some((resolution.range(), resolution.target().clone()))
    }
}

/// The catalog object a resolved target names.
fn object_path(target: &Target, cx: &App, data_source: &Entity<DataSource>) -> Option<ObjectPath> {
    Some(match target {
        Target::Schema { schema } => ObjectPath::Schema {
            schema: schema.clone(),
        },
        Target::Relation { schema, relation } => {
            ObjectPath::relation(schema.clone(), relation.clone())
        }
        Target::Column {
            schema,
            relation,
            column,
        } => ObjectPath::Column {
            schema: schema.clone(),
            relation: relation.clone(),
            column: column.clone(),
        },
        Target::Routine { schema, name } => {
            let source = data_source.read(cx);
            let routine = source
                .catalog()
                .schema(schema)?
                .routines_named(name)
                .next()?;
            ObjectPath::Routine {
                schema: schema.clone(),
                signature: routine.signature().into(),
            }
        }
        Target::Declaration(_) => return None,
    })
}

impl HoverProvider for SqlIntelligence {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        let hover = (|| {
            let data_source = self.data_source.upgrade()?;
            let (range, target) = self.resolution(text, offset, cx)?;
            let target = match target {
                // An alias documents the relation it stands for.
                Target::Declaration(declaration) => self.resolution(text, declaration.start, cx)?.1,
                target => target,
            };
            let path = object_path(&target, cx, &data_source)?;
            let documentation = ObjectRef::new(data_source, path).documentation(cx)?;
            Some(lsp_types::Hover {
                contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: documentation,
                }),
                range: Some(lsp_types::Range::new(
                    text.offset_to_position(range.start),
                    text.offset_to_position(range.end),
                )),
            })
        })();
        Task::ready(Ok(hover))
    }
}

impl DefinitionProvider for SqlIntelligence {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let link = (|| {
            let data_source = self.data_source.upgrade()?;
            let (range, target) = self.resolution(text, offset, cx)?;
            let origin = lsp_types::Range::new(
                text.offset_to_position(range.start),
                text.offset_to_position(range.end),
            );
            let (uri, target_range) = match &target {
                Target::Declaration(declaration) => (
                    Uri::from_str(DOCUMENT).ok()?,
                    lsp_types::Range::new(
                        text.offset_to_position(declaration.start),
                        text.offset_to_position(declaration.end),
                    ),
                ),
                target => {
                    let path = object_path(target, cx, &data_source)?;
                    let mut targets = self.targets.borrow_mut();
                    // Only the last few lookups can still be followed.
                    if targets.len() > 64 {
                        targets.clear();
                    }
                    targets.push(path);
                    (
                        Uri::from_str(&format!("{OBJECT}{}", targets.len() - 1)).ok()?,
                        lsp_types::Range::default(),
                    )
                }
            };
            Some(LocationLink {
                origin_selection_range: Some(origin),
                target_uri: uri,
                target_range,
                target_selection_range: target_range,
            })
        })();
        Task::ready(Ok(link.into_iter().collect()))
    }
}

impl CodeActionProvider for SqlIntelligence {
    fn id(&self) -> SharedString {
        "SqlInspections".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let text = state.read(cx).text().clone();
        let Ok(uri) = Uri::from_str(DOCUMENT) else {
            return Task::ready(Ok(Vec::new()));
        };
        let actions = self
            .inspections
            .borrow()
            .iter()
            .filter(|inspection| {
                let found = inspection.range();
                found.start <= range.end && range.start <= found.end
            })
            .flat_map(|inspection| {
                inspection.fixes().iter().map(|fix| {
                    let edit = TextEdit {
                        range: lsp_types::Range::new(
                            text.offset_to_position(fix.range().start),
                            text.offset_to_position(fix.range().end),
                        ),
                        new_text: fix.replacement().to_string(),
                    };
                    CodeAction {
                        title: fix_title(inspection.problem(), fix.replacement()),
                        kind: Some(CodeActionKind::QUICKFIX),
                        edit: Some(WorkspaceEdit {
                            changes: Some([(uri.clone(), vec![edit])].into_iter().collect()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                })
            })
            .collect();
        Task::ready(Ok(actions))
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let Some(edits) = action
            .edit
            .and_then(|edit| edit.changes)
            .and_then(|changes| changes.into_values().next())
        else {
            return Task::ready(Ok(()));
        };
        let state = state.downgrade();
        window.spawn(cx, async move |cx| {
            state.update_in(cx, |state, window, cx| {
                state.apply_lsp_edits(&edits, window, cx)
            })
        })
    }
}

/// What a problem says, in the interface language.
pub fn problem_message(problem: &Problem) -> SharedString {
    match problem {
        Problem::UnknownSchema { schema } => t!("inspection.unknown_schema", name = schema),
        Problem::UnknownRelation { relation } => t!("inspection.unknown_relation", name = relation),
        Problem::UnknownColumn {
            column,
            relation: Some(relation),
        } => t!(
            "inspection.unknown_column_of",
            name = column,
            relation = relation
        ),
        Problem::UnknownColumn {
            column,
            relation: None,
        } => t!("inspection.unknown_column", name = column),
        Problem::AmbiguousColumn { column, relations } => t!(
            "inspection.ambiguous_column",
            name = column,
            relations = relations.join(", ")
        ),
        Problem::DeleteWithoutWhere => t!("inspection.delete_without_where"),
        Problem::UpdateWithoutWhere => t!("inspection.update_without_where"),
    }
    .into()
}

fn fix_title(problem: &Problem, replacement: &str) -> String {
    match problem {
        Problem::DeleteWithoutWhere | Problem::UpdateWithoutWhere => {
            t!("inspection.add_where").to_string()
        }
        Problem::AmbiguousColumn { .. } => t!("inspection.qualify", name = replacement).to_string(),
        _ => t!("inspection.change_to", name = replacement).to_string(),
    }
}
