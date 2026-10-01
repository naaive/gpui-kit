//! `MenuBarExtra` and its parts: `MenuBarItem`, `MenuBarSection`,
//! `MenuBarSubmenu`, `MenuBarSeparator`.
//!
//! A `menu-bar` command's `render` returns a `MenuBarExtra`, which the
//! launcher shows as an icon in the system tray (the menu bar on macOS) with
//! a menu. An item's `action` is any `Action`, so a menu entry opens a link,
//! copies, or calls back into the script exactly as a list item does.

use anyhow::{Result, bail};
use gpui_kit::AnyElement;
use gpui_shell::{
    ComponentArgument, ComponentDescriptor, ComponentMaterializer, ComponentRegistry,
    MaterializeRequest, RegistryError,
};

use super::{
    action::resolve_action,
    shared::{
        bool_method, carry, describe, element_method, empty_constructor, payload, recorded,
        reject_style, reporting, string_method, strings_constructor, taken,
    },
};
use crate::model::{Image, MenuBarEntry, MenuBarItem, MenuBarModel, MenuBarSection};

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    registry.register(menu_bar_extra())?;
    registry.register(menu_bar_item())?;
    registry.register(menu_bar_section())?;
    registry.register(menu_bar_submenu())?;
    registry.register(menu_bar_separator())?;
    Ok(())
}

/// Reads the entries of a `MenuBarExtra`, `MenuBarSection` or
/// `MenuBarSubmenu`; a separator closes the section so far.
fn entries(
    request: &mut MaterializeRequest<'_>,
    owner: &str,
    sections: bool,
) -> Result<Vec<Vec<Entry>>> {
    let mut groups: Vec<Vec<Entry>> = vec![Vec::new()];
    for mut child in request.take_typed_children()? {
        let name = child.component_name();
        let mut element = request.materialize_child(&mut child)?;
        let entry = match name {
            Some("MenuBarItem") => {
                Entry::Entry(MenuBarEntry::Item(taken(&mut element, "a MenuBarItem")?))
            }
            Some("MenuBarSubmenu") => Entry::Entry(taken(&mut element, "a MenuBarSubmenu")?),
            Some("MenuBarSeparator") => {
                groups.push(Vec::new());
                continue;
            }
            Some("MenuBarSection") if sections => {
                Entry::Section(Box::new(taken(&mut element, "a MenuBarSection")?))
            }
            other => bail!(
                "{owner} accepts MenuBarItem, MenuBarSubmenu{} and MenuBarSeparator children, \
                 not {}",
                if sections { ", MenuBarSection" } else { "" },
                describe(other)
            ),
        };
        groups.last_mut().expect("starts with one").push(entry);
    }
    Ok(groups)
}

enum Entry {
    Entry(MenuBarEntry),
    Section(Box<MenuBarSection>),
}

// MARK: MenuBarExtra

#[derive(Clone)]
enum ExtraOp {
    Icon(String),
    Title(String),
    Tooltip(String),
    Loading(bool),
}

fn menu_bar_extra() -> ComponentDescriptor {
    ComponentDescriptor::new("MenuBarExtra", reporting(ExtraMaterializer))
        .with_documentation(
            "What a `menu-bar` command's `render` returns: an icon in the system tray (the menu \
             bar on macOS) and its menu. Children are `MenuBarItem`, `MenuBarSubmenu`, \
             `MenuBarSection` and `MenuBarSeparator`.",
        )
        .with_constructors(vec![empty_constructor("MenuBarExtra")])
        .with_methods(vec![
            string_method(
                "icon",
                "A Lucide icon name, or a PNG or SVG inside the extension.",
                ExtraOp::Icon,
            ),
            string_method(
                "title",
                "Text beside the icon on macOS; in the tooltip elsewhere. Keep it short, such \
                 as a count.",
                ExtraOp::Title,
            ),
            string_method(
                "tooltip",
                "Shown when the pointer rests on the icon.",
                ExtraOp::Tooltip,
            ),
            bool_method(
                "loading",
                "Shows that the menu is being filled; the tooltip says so.",
                ExtraOp::Loading,
            ),
        ])
}

struct ExtraMaterializer;

impl ComponentMaterializer for ExtraMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "MenuBarExtra")?;
        let mut model = MenuBarModel::new();
        for op in recorded::<ExtraOp>(&request) {
            model = match op {
                ExtraOp::Icon(icon) => model.with_icon(Image::parse(&icon)),
                ExtraOp::Title(title) => model.with_title(title),
                ExtraOp::Tooltip(tooltip) => model.with_tooltip(tooltip),
                ExtraOp::Loading(loading) => model.with_loading(loading),
            };
        }
        for group in entries(&mut request, "MenuBarExtra", true)? {
            let mut loose = MenuBarSection::new();
            let mut has_loose = false;
            for entry in group {
                match entry {
                    Entry::Entry(entry) => {
                        loose = loose.with_entry(entry);
                        has_loose = true;
                    }
                    Entry::Section(section) => {
                        if has_loose {
                            model = model.with_section(std::mem::take(&mut loose));
                            has_loose = false;
                        }
                        model = model.with_section(*section);
                    }
                }
            }
            if has_loose {
                model = model.with_section(loose);
            }
        }
        Ok(carry(model))
    }
}

// MARK: MenuBarItem

#[derive(Clone)]
struct Title(String);

#[derive(Clone)]
enum ItemOp {
    Subtitle(String),
    Checked(bool),
    Action(ComponentArgument),
}

fn menu_bar_item() -> ComponentDescriptor {
    ComponentDescriptor::new("MenuBarItem", reporting(ItemMaterializer))
        .with_documentation(
            "One line of the menu. Give it an `action`; without one it is a disabled label.",
        )
        .with_constructors(vec![strings_constructor(
            "MenuBarItem",
            &["title"],
            &["title"],
            |mut values| Title(values.remove(0)),
        )])
        .with_methods(vec![
            string_method(
                "subtitle",
                "Secondary text at the end of the line.",
                ItemOp::Subtitle,
            ),
            bool_method(
                "checked",
                "Draws a check mark beside the item when true.",
                ItemOp::Checked,
            ),
            element_method(
                "action",
                "action",
                "The `Action` choosing the item performs.",
                ItemOp::Action,
            ),
        ])
}

struct ItemMaterializer;

impl ComponentMaterializer for ItemMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "MenuBarItem")?;
        let Title(title) = payload(&request, "MenuBarItem")?;
        let mut item = MenuBarItem::new(title);
        for op in recorded::<ItemOp>(&request) {
            item = match op {
                ItemOp::Subtitle(subtitle) => item.with_subtitle(subtitle),
                ItemOp::Checked(checked) => item.with_checked(checked),
                ItemOp::Action(action) => {
                    item.with_action(resolve_action(&mut request, &action, "MenuBarItem")?)
                }
            };
        }
        Ok(carry(item))
    }
}

// MARK: MenuBarSection, MenuBarSubmenu, MenuBarSeparator

fn menu_bar_section() -> ComponentDescriptor {
    ComponentDescriptor::new("MenuBarSection", reporting(SectionMaterializer))
        .with_documentation(
            "A group of items set apart from the others, optionally titled. Children are \
             `MenuBarItem` and `MenuBarSubmenu`.",
        )
        .with_constructors(vec![strings_constructor(
            "MenuBarSection",
            &["title"],
            &[],
            |mut values| Title(values.remove(0)),
        )])
}

struct SectionMaterializer;

impl ComponentMaterializer for SectionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "MenuBarSection")?;
        let Title(title) = payload(&request, "MenuBarSection")?;
        let mut section = MenuBarSection::new();
        if !title.trim().is_empty() {
            section = section.with_title(title);
        }
        for entry in entries(&mut request, "MenuBarSection", false)?
            .into_iter()
            .flatten()
        {
            if let Entry::Entry(entry) = entry {
                section = section.with_entry(entry);
            }
        }
        Ok(carry(section))
    }
}

fn menu_bar_submenu() -> ComponentDescriptor {
    ComponentDescriptor::new("MenuBarSubmenu", reporting(SubmenuMaterializer))
        .with_documentation(
            "A nested menu. Children are `MenuBarItem`, `MenuBarSubmenu` and `MenuBarSeparator`.",
        )
        .with_constructors(vec![strings_constructor(
            "MenuBarSubmenu",
            &["title"],
            &["title"],
            |mut values| Title(values.remove(0)),
        )])
}

struct SubmenuMaterializer;

impl ComponentMaterializer for SubmenuMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "MenuBarSubmenu")?;
        let Title(title) = payload(&request, "MenuBarSubmenu")?;
        let groups = entries(&mut request, "MenuBarSubmenu", false)?;
        let count = groups.len();
        let mut entries = Vec::new();
        for (ix, group) in groups.into_iter().enumerate() {
            entries.extend(group.into_iter().filter_map(|entry| match entry {
                Entry::Entry(entry) => Some(entry),
                Entry::Section(_) => None,
            }));
            if ix + 1 < count {
                entries.push(MenuBarEntry::Separator);
            }
        }
        Ok(carry(MenuBarEntry::Submenu {
            title: title.into(),
            entries,
        }))
    }
}

fn menu_bar_separator() -> ComponentDescriptor {
    ComponentDescriptor::new("MenuBarSeparator", reporting(SeparatorMaterializer))
        .with_documentation("A line between groups of entries.")
        .with_constructors(vec![empty_constructor("MenuBarSeparator")])
}

struct SeparatorMaterializer;

impl ComponentMaterializer for SeparatorMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "MenuBarSeparator")?;
        Ok(carry(()))
    }
}
