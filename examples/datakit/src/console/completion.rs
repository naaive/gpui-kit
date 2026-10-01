use anyhow::Result;
use datakit_sql::{CandidateCategory, CompletionRequest, complete, templates::templates_for};
use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Task, WeakEntity, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};

use crate::datasource::{CatalogRequest, DataSource};

/// Completion from the data source's catalog, through [`datakit_sql`].
///
/// Asking for completion is also what loads metadata: the schema list on the
/// first request, and the relations of a schema the moment a statement
/// needs them, so the next keystroke can offer them.
pub struct SqlCompletion {
    data_source: WeakEntity<DataSource>,
}

impl SqlCompletion {
    pub fn new(data_source: WeakEntity<DataSource>) -> Self {
        Self { data_source }
    }
}

impl CompletionProvider for SqlCompletion {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let Some(data_source) = self.data_source.upgrade() else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        data_source.update(cx, |data_source, cx| {
            data_source.ensure(CatalogRequest::Schemas, cx)
        });
        let (catalog, dialect) = {
            let data_source = data_source.read(cx);
            (data_source.catalog().clone(), data_source.dialect())
        };
        let rope = rope.clone();
        let weak = self.data_source.clone();
        cx.spawn(async move |cx| {
            let text = rope.to_string();
            let completions = cx
                .background_spawn(async move {
                    complete(&CompletionRequest::new(&text, offset, &catalog, &*dialect))
                })
                .await;
            for schema in completions.missing_schemas() {
                let _ = weak.update(cx, |data_source, cx| {
                    data_source.ensure(CatalogRequest::Objects(schema.clone()), cx)
                });
            }
            let replace = completions.replace_range();
            let range = lsp_types::Range::new(
                rope.offset_to_position(replace.start),
                rope.offset_to_position(replace.end),
            );
            let prefix = rope.slice(replace.clone()).to_string();
            // Templates whose abbreviation is typed come first.
            let templates = templates_for(&prefix).map(|template| CompletionItem {
                label: template.abbreviation().to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: Some(template.description().to_string()),
                sort_text: Some("0".into()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: template.body(),
                })),
                ..Default::default()
            });
            let items = templates
                .collect::<Vec<_>>()
                .into_iter()
                .chain(
                    completions
                        .candidates()
                        .iter()
                        .enumerate()
                        .map(|(ix, candidate)| CompletionItem {
                            label: candidate.label().to_string(),
                            kind: Some(match candidate.category() {
                                CandidateCategory::Keyword => CompletionItemKind::KEYWORD,
                                CandidateCategory::Function => CompletionItemKind::FUNCTION,
                                CandidateCategory::Schema => CompletionItemKind::MODULE,
                                CandidateCategory::Table => CompletionItemKind::CLASS,
                                CandidateCategory::View => CompletionItemKind::INTERFACE,
                                CandidateCategory::Column => CompletionItemKind::FIELD,
                                CandidateCategory::Alias => CompletionItemKind::VARIABLE,
                                CandidateCategory::JoinCondition => CompletionItemKind::SNIPPET,
                            }),
                            detail: candidate.detail().map(str::to_string),
                            // The engine already ranked them; keep its order.
                            sort_text: Some(format!("{ix:05}")),
                            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                range,
                                new_text: candidate.insert_text().to_string(),
                            })),
                            ..Default::default()
                        }),
                )
                .collect();
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }
}
