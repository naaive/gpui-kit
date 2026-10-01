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
        if data_source.read(cx).dialect().statements_are_lines() {
            return Task::ready(Ok(CompletionResponse::Array(command_completions(
                rope,
                offset,
                &data_source,
                cx,
            ))));
        }
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

/// Completion for a language of commands, one a line: the command at the
/// start of a line, keys of the session's database after it.
fn command_completions(
    rope: &Rope,
    offset: usize,
    data_source: &gpui_kit::Entity<DataSource>,
    cx: &mut App,
) -> Vec<CompletionItem> {
    let text = rope.to_string();
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |ix| ix + 1);
    let line = &text[line_start..offset];
    let word_start = line
        .rfind(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .map_or(0, |ix| ix + 1);
    let prefix = &line[word_start..];
    let first_word = line[..word_start].trim().is_empty();
    let range = lsp_types::Range::new(
        rope.offset_to_position(line_start + word_start),
        rope.offset_to_position(offset),
    );
    let item = |label: String, kind: CompletionItemKind, detail: Option<String>| CompletionItem {
        label: label.clone(),
        kind: Some(kind),
        detail,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: label,
        })),
        ..Default::default()
    };
    if first_word {
        let prefix = prefix.to_uppercase();
        return data_source
            .read(cx)
            .dialect()
            .keywords()
            .iter()
            .filter(|command| command.starts_with(&prefix))
            .map(|command| item(command.to_string(), CompletionItemKind::KEYWORD, None))
            .collect();
    }
    let schema = data_source
        .read(cx)
        .catalog()
        .search_path()
        .first()
        .cloned();
    let Some(schema) = schema else {
        return Vec::new();
    };
    if data_source.read(cx).loaded_schema(&schema).is_none() {
        data_source.update(cx, |data_source, cx| {
            data_source.ensure(CatalogRequest::Objects(schema.clone()), cx)
        });
        return Vec::new();
    }
    let source = data_source.read(cx);
    let dialect = source.dialect();
    source
        .loaded_schema(&schema)
        .and_then(|schema| schema.relations())
        .unwrap_or_default()
        .iter()
        .filter(|key| key.name().starts_with(prefix))
        .take(200)
        .map(|key| {
            item(
                dialect.quote_identifier(&key.name()),
                CompletionItemKind::VALUE,
                key.comment().map(str::to_string),
            )
        })
        .collect()
}
