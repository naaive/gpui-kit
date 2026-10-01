//! Importing a CSV or TSV file into a table.
//!
//! File columns go to the table columns of the same name, ignoring case, or
//! by position when the file has no header. Rows are inserted in batches
//! inside one transaction, so a failed import leaves the table as it was.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_driver::Value;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    notification::Notification,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use rust_i18n::t;

use crate::{
    datasource::{CatalogRequest, DataSource, describe_error},
    format,
    objects::{CatalogObject, ObjectPath, ObjectRef},
};

/// How many rows the preview shows.
const PREVIEW_ROWS: usize = 8;

/// A file read for import: its columns and rows.
#[derive(Clone, Default)]
struct Parsed {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Read `path` as delimited text. The delimiter is a tab for `.tsv` and
/// `.tab` files and a comma otherwise.
fn parse(path: &PathBuf, has_header: bool) -> Result<Parsed> {
    let delimiter = match path.extension().and_then(|e| e.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("tsv") || extension == "tab" => b'\t',
        _ => b',',
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("Couldn’t open {}", path.display()))?;
    let mut records = reader.records();
    let mut parsed = Parsed::default();
    if has_header && let Some(first) = records.next() {
        parsed.header = first?
            .iter()
            .map(|field| field.trim_start_matches('\u{feff}').to_string())
            .collect();
    }
    for record in records {
        parsed
            .rows
            .push(record?.iter().map(str::to_string).collect());
    }
    if parsed.header.is_empty() {
        let width = parsed.rows.first().map_or(0, Vec::len);
        parsed.header = (1..=width).map(|n| format!("column{n}")).collect();
    }
    Ok(parsed)
}

/// For each file column, the table column it goes to, if any.
fn map_columns(header: &[String], columns: &[Arc<str>], by_name: bool) -> Vec<Option<usize>> {
    header
        .iter()
        .enumerate()
        .map(|(ix, name)| {
            if by_name {
                columns
                    .iter()
                    .position(|column| column.eq_ignore_ascii_case(name.trim()))
            } else {
                (ix < columns.len()).then_some(ix)
            }
        })
        .collect()
}

/// The import dialog's content. Its footer is drawn by the dialog, apart
/// from this view, so every change of state also refreshes the window.
pub struct ImportDialog {
    object: ObjectRef,
    columns: Vec<Arc<str>>,
    path: Option<PathBuf>,
    has_header: bool,
    empty_is_null: bool,
    parsed: Option<Parsed>,
    problem: Option<SharedString>,
    task: Option<Task<()>>,
}

impl ImportDialog {
    pub fn open(object: &ObjectRef, window: &mut Window, cx: &mut App) {
        let ObjectPath::Relation { schema, relation } = object.path() else {
            return;
        };
        let columns: Vec<Arc<str>> = match object
            .path()
            .resolve(object.data_source().read(cx).catalog())
        {
            Some(CatalogObject::Relation(relation)) => relation
                .columns()
                .iter()
                .map(|column| column.name())
                .collect(),
            _ => Vec::new(),
        };
        let title: SharedString = t!("import.title", table = relation).into();
        let _ = schema;
        let dialog = cx.new(|_| Self {
            object: object.clone(),
            columns,
            path: None,
            has_header: true,
            empty_is_null: true,
            parsed: None,
            problem: None,
            task: None,
        });
        window.open_dialog(cx, {
            let dialog = dialog.clone();
            move |modal, _, cx| {
                let importing = dialog.read(cx).task.is_some();
                modal
                    .title(title.clone())
                    .w(px(760.))
                    .child(dialog.clone())
                    .footer(
                        DialogFooter::new().child(
                            h_flex()
                                .gap_2()
                                .child(
                                    DialogClose::new().child(
                                        Button::new("cancel")
                                            .outline()
                                            .label(t!("common.cancel").to_string()),
                                    ),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("import")
                                            .primary()
                                            .loading(importing)
                                            .label(t!("import.import").to_string()),
                                    ),
                                ),
                        ),
                    )
                    .on_ok({
                        let dialog = dialog.clone();
                        move |_, window, cx| {
                            dialog.update(cx, |dialog, cx| dialog.import(window, cx));
                            false
                        }
                    })
            }
        });
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                let _ = this.update(cx, |this, _| this.task = None);
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                this.path = paths.into_iter().next();
                this.read_file(cx);
            });
        }));
    }

    fn read_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let has_header = self.has_header;
        self.task = Some(cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move { parse(&path, has_header) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                match parsed {
                    Ok(parsed) => {
                        this.problem = None;
                        this.parsed = Some(parsed);
                    }
                    Err(error) => {
                        this.parsed = None;
                        this.problem = Some(describe_error(&error));
                    }
                }
                cx.notify();
                cx.refresh_windows();
            });
        }));
        cx.notify();
        cx.refresh_windows();
    }

    fn mapping(&self) -> Vec<Option<usize>> {
        match &self.parsed {
            Some(parsed) => map_columns(&parsed.header, &self.columns, self.has_header),
            None => Vec::new(),
        }
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(parsed) = self.parsed.clone() else {
            return;
        };
        let ObjectPath::Relation { schema, relation } = self.object.path().clone() else {
            return;
        };
        let mapping = self.mapping();
        let targets: Vec<(usize, Arc<str>)> = mapping
            .iter()
            .enumerate()
            .filter_map(|(file_ix, table_ix)| {
                Some((file_ix, self.columns.get((*table_ix)?)?.clone()))
            })
            .collect();
        if targets.is_empty() {
            self.problem = Some(t!("import.no_columns").into());
            cx.notify();
            cx.refresh_windows();
            return;
        }
        let data_source: Entity<DataSource> = self.object.data_source().clone();
        let dialect = data_source.read(cx).dialect();
        let empty_is_null = self.empty_is_null;
        let names: Vec<&str> = targets.iter().map(|(_, name)| &**name).collect();
        let rows: Vec<Vec<Value>> = parsed
            .rows
            .iter()
            .map(|row| {
                targets
                    .iter()
                    .map(|(file_ix, _)| match row.get(*file_ix) {
                        Some(field) if !(field.is_empty() && empty_is_null) => {
                            Value::Text(field.as_str().into())
                        }
                        _ => Value::Null,
                    })
                    .collect()
            })
            .collect();
        let count = rows.len();
        let mut statements: Vec<String> = rows
            .chunks(dialect.insert_batch_size().max(1))
            .map(|batch| dialect.insert_rows(&schema, &relation, &names, batch))
            .collect();
        if dialect.supports_transactions() {
            statements.insert(0, dialect.begin_transaction().to_string());
            statements.push(dialect.commit().to_string());
        }
        let task = data_source.read(cx).run_statements(statements, cx);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.task = None;
                match result {
                    Ok(()) => {
                        data_source.update(cx, |data_source, cx| {
                            data_source.request(CatalogRequest::Objects(schema.clone()), cx)
                        });
                        window.close_dialog(cx);
                        window.push_notification(
                            Notification::success(
                                t!(
                                    "import.done",
                                    count = format::count(count),
                                    table = relation
                                )
                                .to_string(),
                            ),
                            cx,
                        );
                    }
                    Err(error) => this.problem = Some(describe_error(&error)),
                }
                cx.notify();
                cx.refresh_windows();
            });
        }));
        cx.notify();
        cx.refresh_windows();
    }
}

impl Render for ImportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mapping = self.mapping();
        let file: SharedString = match &self.path {
            Some(path) => path.display().to_string().into(),
            None => t!("import.no_file").into(),
        };
        v_flex()
            .gap_3()
            .text_sm()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if self.path.is_some() {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(file),
                    )
                    .child(
                        Button::new("choose-file")
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t!("import.choose").to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        Checkbox::new("has-header")
                            .label(t!("import.has_header").to_string())
                            .checked(self.has_header)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.has_header = *checked;
                                this.read_file(cx);
                            })),
                    )
                    .child(
                        Checkbox::new("empty-is-null")
                            .label(t!("import.empty_is_null").to_string())
                            .checked(self.empty_is_null)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.empty_is_null = *checked;
                                cx.notify();
                                cx.refresh_windows();
                            })),
                    ),
            )
            .when_some(self.parsed.as_ref(), |body, parsed| {
                let columns = parsed.header.len();
                body.child(
                    div().text_xs().text_color(theme.muted_foreground).child(
                        t!(
                            "import.summary",
                            rows = format::count(parsed.rows.len()),
                            mapped = mapping.iter().flatten().count(),
                            columns = columns
                        )
                        .to_string(),
                    ),
                )
                .child(
                    div()
                        .id("import-preview")
                        .max_h(px(260.))
                        .overflow_scrollbar()
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius)
                        .child(
                            v_flex()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .child(h_flex().bg(theme.muted).children(
                                    parsed.header.iter().zip(&mapping).map(|(name, target)| {
                                        let target: SharedString = match target {
                                            Some(ix) => format!("→ {}", self.columns[*ix]).into(),
                                            None => t!("import.skipped").into(),
                                        };
                                        v_flex()
                                            .w(rems(9.))
                                            .flex_none()
                                            .px_1()
                                            .py_0p5()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .child(SharedString::from(name.clone())),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_color(if target.starts_with('→') {
                                                        theme.success
                                                    } else {
                                                        theme.muted_foreground
                                                    })
                                                    .child(target),
                                            )
                                    }),
                                ))
                                .children(parsed.rows.iter().take(PREVIEW_ROWS).map(|row| {
                                    h_flex().border_t_1().border_color(theme.border).children(
                                        (0..columns).map(|ix| {
                                            div()
                                                .w(rems(9.))
                                                .flex_none()
                                                .px_1()
                                                .py_0p5()
                                                .truncate()
                                                .child(SharedString::from(
                                                    row.get(ix).cloned().unwrap_or_default(),
                                                ))
                                        }),
                                    )
                                })),
                        ),
                )
            })
            .when_some(self.problem.clone(), |body, problem| {
                body.child(div().text_color(theme.danger).child(problem))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_map_by_name_ignoring_case_or_by_position() {
        let columns: Vec<Arc<str>> = vec!["id".into(), "email".into(), "name".into()];
        let header = vec!["Name".to_string(), "ID".to_string(), "extra".to_string()];
        assert_eq!(
            map_columns(&header, &columns, true),
            vec![Some(2), Some(0), None]
        );
        assert_eq!(
            map_columns(&header, &columns, false),
            vec![Some(0), Some(1), Some(2)]
        );
    }

    #[test]
    fn a_file_is_read_with_its_header() {
        let directory = std::env::temp_dir().join(format!("datakit-import-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("people.csv");
        std::fs::write(&path, "\u{feff}id,name\n1,\"Lovelace, Ada\"\n2,\n").unwrap();
        let parsed = parse(&path, true).unwrap();
        assert_eq!(parsed.header, ["id", "name"]);
        assert_eq!(parsed.rows, vec![vec!["1", "Lovelace, Ada"], vec!["2", ""]]);
        let _ = std::fs::remove_dir_all(directory);
    }
}
