//! Renaming a table, view or column: in the database, and wherever the open
//! consoles name it, as DataGrip's Rename refactoring does.

use datakit_sql::Target;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{App, Entity, Window};
use rust_i18n::t;

use crate::{
    console::Sessions,
    datasource::{CatalogRequest, DataSource, describe_error},
    objects::ObjectRef,
    prompt::prompt_text,
};

/// Ask for `object`'s new name, rename it, and change the consoles on its
/// data source to use the new name.
pub fn rename_object(object: &ObjectRef, window: &mut Window, cx: &mut App) {
    let Some(target) = object.target() else {
        return;
    };
    let name = object.path().name().to_string();
    let object = object.clone();
    prompt_text(
        t!("rename.title", name = name).into(),
        t!("rename.label").into(),
        &name.clone(),
        t!("rename.confirm").into(),
        move |new_name, window, cx| {
            let new_name = new_name.trim().to_string();
            if new_name.is_empty() || new_name == name {
                return;
            }
            let Some(statement) = object.rename_statement(&new_name, cx) else {
                return;
            };
            let data_source = object.data_source().clone();
            let schema = object.path().schema().clone();
            let target = target.clone();
            let task = data_source.read(cx).run_statements(vec![statement], cx);
            window
                .spawn(cx, async move |cx| {
                    let result = task.await;
                    let _ = cx.update(|window, cx| match result {
                        Ok(()) => {
                            let updated =
                                rename_in_consoles(&data_source, &target, &new_name, window, cx);
                            data_source.update(cx, |data_source, cx| {
                                data_source.request(CatalogRequest::Objects(schema), cx)
                            });
                            if updated > 0 {
                                window.push_notification(
                                    Notification::info(
                                        t!("rename.updated", count = updated).to_string(),
                                    ),
                                    cx,
                                );
                            }
                        }
                        Err(error) => window.push_notification(
                            Notification::error(describe_error(&error).to_string()),
                            cx,
                        ),
                    });
                })
                .detach();
        },
        window,
        cx,
    );
}

/// Change every name in the consoles on `data_source` that refers to
/// `target` to `new_name`. Returns how many names changed.
///
/// The catalog still describes the database before the rename, which is
/// what the consoles' text was written against.
fn rename_in_consoles(
    data_source: &Entity<DataSource>,
    target: &Target,
    new_name: &str,
    window: &mut Window,
    cx: &mut App,
) -> usize {
    let source = data_source.read(cx);
    let catalog = source.catalog().clone();
    let dialect = source.dialect();
    let replacement = dialect.quote_identifier(new_name);
    let consoles: Vec<_> = Sessions::global(cx)
        .read(cx)
        .consoles()
        .into_iter()
        .filter(|console| console.read(cx).data_source() == Some(data_source))
        .collect();
    consoles
        .into_iter()
        .map(|console| {
            console.update(cx, |console, cx| {
                console.replace_usages(target, &catalog, &*dialect, &replacement, window, cx)
            })
        })
        .sum()
}
