//! `Detail` and its metadata: `MetadataLabel`, `MetadataLink`, `MetadataTags`,
//! `MetadataSeparator`.
//!
//! A `Detail` is a page of its own when `render` returns it, and the side
//! panel of a list item when passed to `ListItem.detail`.

use anyhow::{Result, bail};
use gpui_kit::AnyElement;
use gpui_shell::{
    ComponentArgument, ComponentDescriptor, ComponentMaterializer, ComponentRegistry,
    MaterializeRequest, RegistryError,
};

use super::{
    action::resolve_panel,
    shared::{
        bool_method, carry, describe, element_method, payload, recorded, reject_style, reporting,
        strings_constructor, tag_method, taken,
    },
};
use crate::model::{DetailModel, Metadata, MetadataValue, PageModel, Tag, Tone};

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    registry.register(detail())?;
    registry.register(metadata_label())?;
    registry.register(metadata_link())?;
    registry.register(metadata_tags())?;
    registry.register(metadata_separator())?;
    Ok(())
}

// MARK: Detail

#[derive(Clone)]
struct Markdown(String);

#[derive(Clone)]
enum DetailOp {
    Loading(bool),
    Actions(ComponentArgument),
}

fn detail() -> ComponentDescriptor {
    ComponentDescriptor::new("Detail", reporting(DetailMaterializer))
        .with_documentation(
            "One object in full: a Markdown body with a metadata column. Return it from \
             `render` for a page of its own, or pass it to `ListItem.detail`. Children are \
             `MetadataLabel`, `MetadataLink`, `MetadataTags` and `MetadataSeparator`.",
        )
        .with_constructors(vec![strings_constructor(
            "Detail",
            &["markdown"],
            &[],
            |mut values| Markdown(values.remove(0)),
        )])
        .with_methods(vec![
            bool_method(
                "loading",
                "Shows that the content is on its way.",
                DetailOp::Loading,
            ),
            element_method(
                "actions",
                "panel",
                "The page's actions, an `ActionPanel`; shown by Cmd-K and Enter.",
                DetailOp::Actions,
            ),
        ])
}

struct DetailMaterializer;

impl ComponentMaterializer for DetailMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "Detail")?;
        let Markdown(markdown) = payload(&request, "Detail")?;
        let mut detail = DetailModel::new(markdown);
        for op in recorded::<DetailOp>(&request) {
            detail = match op {
                DetailOp::Loading(loading) => detail.with_loading(loading),
                DetailOp::Actions(panel) => {
                    detail.with_actions(resolve_panel(&mut request, &panel, "Detail")?)
                }
            };
        }
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            detail = match name {
                Some("MetadataLabel" | "MetadataLink" | "MetadataTags" | "MetadataSeparator") => {
                    detail.with_metadata(taken(&mut element, "metadata")?)
                }
                other => bail!(
                    "Detail accepts MetadataLabel, MetadataLink, MetadataTags and \
                     MetadataSeparator children, not {}",
                    describe(other)
                ),
            };
        }
        Ok(carry(PageModel::Detail(detail)))
    }
}

/// Reads the `detail(detail)` argument of a list item.
pub(super) fn resolve_detail(
    request: &mut MaterializeRequest<'_>,
    argument: &ComponentArgument,
) -> Result<DetailModel> {
    let mut element = request.resolve_element(argument)?;
    match taken::<PageModel>(&mut element, "a Detail") {
        Ok(PageModel::Detail(detail)) => Ok(detail),
        _ => bail!("ListItem.detail expects a Detail"),
    }
}

// MARK: Metadata

/// Each metadata node materializes straight from its constructor, so one
/// materializer serves all of them.
#[derive(Clone)]
struct MetadataHead(Metadata);

#[derive(Clone)]
struct TagOp(String, Tone);

struct MetadataMaterializer(&'static str);

impl ComponentMaterializer for MetadataMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), self.0)?;
        let MetadataHead(metadata) = payload(&request, self.0)?;
        let tags: Vec<Tag> = recorded::<TagOp>(&request)
            .into_iter()
            .map(|TagOp(text, tone)| Tag::new(text).with_tone(tone))
            .collect();
        let metadata = match metadata.value() {
            MetadataValue::Tags(_) => {
                Metadata::new(metadata.label().clone(), MetadataValue::Tags(tags))
            }
            _ => metadata,
        };
        anyhow::ensure!(
            request.children_len() == 0,
            "{} does not take children",
            self.0
        );
        Ok(carry(metadata))
    }
}

fn metadata_label() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "MetadataLabel",
        reporting(MetadataMaterializer("MetadataLabel")),
    )
    .with_documentation("A labelled line of text in a `Detail`'s metadata.")
    .with_constructors(vec![strings_constructor(
        "MetadataLabel",
        &["label", "text"],
        &["label"],
        |values| {
            MetadataHead(Metadata::new(
                values[0].clone(),
                MetadataValue::Text(values[1].clone().into()),
            ))
        },
    )])
}

fn metadata_link() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "MetadataLink",
        reporting(MetadataMaterializer("MetadataLink")),
    )
    .with_documentation("A labelled link in a `Detail`'s metadata; opens `url` when clicked.")
    .with_constructors(vec![strings_constructor(
        "MetadataLink",
        &["label", "text", "url"],
        &["label", "url"],
        |values| {
            MetadataHead(Metadata::new(
                values[0].clone(),
                MetadataValue::Link {
                    text: values[1].clone().into(),
                    url: values[2].clone().into(),
                },
            ))
        },
    )])
}

fn metadata_tags() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "MetadataTags",
        reporting(MetadataMaterializer("MetadataTags")),
    )
    .with_documentation("A labelled row of tags in a `Detail`'s metadata; add them with `tag`.")
    .with_constructors(vec![strings_constructor(
        "MetadataTags",
        &["label"],
        &["label"],
        |values| {
            MetadataHead(Metadata::new(
                values[0].clone(),
                MetadataValue::Tags(Vec::new()),
            ))
        },
    )])
    .with_methods(vec![tag_method(
        "Adds a tag. `tone` is neutral (the default), accent, success, warning or danger.",
        TagOp,
    )])
}

fn metadata_separator() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "MetadataSeparator",
        reporting(MetadataMaterializer("MetadataSeparator")),
    )
    .with_documentation("A line between groups of a `Detail`'s metadata.")
    .with_constructors(vec![gpui_shell::ConstructorDescriptor::new(
        "MetadataSeparator",
        vec![],
        |_| {
            Ok(gpui_shell::ComponentPayload::new(MetadataHead(
                Metadata::new("", MetadataValue::Separator),
            )))
        },
    )])
}
