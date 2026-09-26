use gpui_kit::SharedString;

use crate::{
    model::{Item, ItemId, ListModel},
    search::score_item,
};

/// One visible line of a list.
#[derive(Clone, Debug)]
pub enum Row {
    Header(SharedString),
    Item(Item),
}

/// The rows a list shows for a query, and the selection moving over them.
///
/// When the list filters, items that do not match are dropped and each
/// section is ordered by score; a section left empty loses its header too.
#[derive(Debug, Default)]
pub struct Rows {
    rows: Vec<Row>,
}

impl Rows {
    pub fn new(list: &ListModel, query: &str) -> Self {
        let filter = list.is_filtering() && !query.trim().is_empty();
        let mut rows = Vec::new();
        for section in list.sections() {
            let mut items: Vec<(u32, &Item)> = section
                .items()
                .iter()
                .filter_map(|item| match filter {
                    true => score_item(query, item).map(|score| (score, item)),
                    false => Some((0, item)),
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            // Stable, so equal scores keep the author's order.
            items.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            if let Some(title) = section.title() {
                rows.push(Row::Header(title.clone()));
            }
            rows.extend(items.into_iter().map(|(_, item)| Row::Item(item.clone())));
        }
        Self { rows }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn has_items(&self) -> bool {
        self.rows.iter().any(|row| matches!(row, Row::Item(_)))
    }

    /// The row index of the selected item.
    ///
    /// The selection is kept by id, so it survives a page reloading its items.
    /// When the selected item is gone, or nothing was selected, the first item
    /// is selected: a launcher always has something `Enter` will do.
    pub fn selected_index(&self, selected: Option<&ItemId>) -> Option<usize> {
        selected
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|row| matches!(row, Row::Item(item) if item.id() == id))
            })
            .or_else(|| self.first_item())
    }

    pub fn item(&self, index: usize) -> Option<&Item> {
        match self.rows.get(index)? {
            Row::Item(item) => Some(item),
            Row::Header(_) => None,
        }
    }

    /// The item `delta` items away from the current selection, skipping
    /// headers and stopping at either end.
    pub fn step(&self, selected: Option<&ItemId>, delta: isize) -> Option<&Item> {
        let items: Vec<&Item> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Item(item) => Some(item),
                Row::Header(_) => None,
            })
            .collect();
        let current = self
            .selected_index(selected)
            .and_then(|ix| self.item(ix))
            .and_then(|item| items.iter().position(|other| other.id() == item.id()))?;
        let next = current
            .saturating_add_signed(delta)
            .min(items.len().saturating_sub(1));
        items.get(next).copied()
    }

    fn first_item(&self) -> Option<usize> {
        self.rows.iter().position(|row| matches!(row, Row::Item(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Section;

    fn item(id: &str, title: &str) -> Item {
        Item::new(ItemId::new(id), title)
    }

    fn list() -> ListModel {
        ListModel::new()
            .with_section(
                Section::new()
                    .with_title("Apps")
                    .with_item(item("code", "Visual Studio Code"))
                    .with_item(item("term", "Terminal")),
            )
            .with_section(
                Section::new()
                    .with_title("Links")
                    .with_item(item("docs", "Documentation")),
            )
    }

    fn ids(rows: &Rows) -> Vec<String> {
        rows.rows()
            .iter()
            .map(|row| match row {
                Row::Header(title) => format!("# {title}"),
                Row::Item(item) => item.id().as_str().to_owned(),
            })
            .collect()
    }

    #[test]
    fn test_filtering_drops_unmatched_items_and_empty_sections() {
        assert_eq!(
            ids(&Rows::new(&list(), "")),
            ["# Apps", "code", "term", "# Links", "docs"]
        );
        assert_eq!(ids(&Rows::new(&list(), "term")), ["# Apps", "term"]);
        assert!(!Rows::new(&list(), "zzz").has_items());
    }

    #[test]
    fn test_a_list_that_searches_itself_is_not_filtered() {
        let rows = Rows::new(&list().with_filtering(false), "zzz");
        assert_eq!(rows.rows().len(), 5);
    }

    #[test]
    fn test_selection_follows_the_item_and_falls_back_to_the_first() {
        let rows = Rows::new(&list(), "");
        assert_eq!(rows.selected_index(None), Some(1));
        assert_eq!(rows.selected_index(Some(&ItemId::new("docs"))), Some(4));
        assert_eq!(rows.selected_index(Some(&ItemId::new("gone"))), Some(1));

        // Filtering moves the item to another row; the selection follows it.
        let filtered = Rows::new(&list(), "docu");
        assert_eq!(filtered.selected_index(Some(&ItemId::new("docs"))), Some(1));
    }

    #[test]
    fn test_stepping_skips_headers_and_stops_at_the_ends() {
        let rows = Rows::new(&list(), "");
        let term = ItemId::new("term");
        assert_eq!(rows.step(Some(&term), 1).unwrap().id().as_str(), "docs");
        assert_eq!(rows.step(Some(&term), -1).unwrap().id().as_str(), "code");
        assert_eq!(rows.step(Some(&term), 10).unwrap().id().as_str(), "docs");
        assert_eq!(rows.step(Some(&term), -10).unwrap().id().as_str(), "code");
        assert!(Rows::new(&list(), "zzz").step(None, 1).is_none());
    }
}
