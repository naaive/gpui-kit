use std::ops::Range;

use gpui_kit::SharedString;

use crate::{
    model::{Item, ItemId, Layout, ListModel},
    search::score_item,
};

/// One visible entry of a list: a section header or an item.
#[derive(Clone, Debug)]
pub enum Row {
    Header {
        title: SharedString,
        subtitle: Option<SharedString>,
    },
    /// Boxed: an item is large, and most rows of a long list are items.
    Item(Box<Item>),
}

/// One drawn line: a section header, or the items that share a line.
///
/// In a list every item is a line of its own; in a grid a line holds up to
/// `columns` items of one section. Lines are what the window virtualizes and
/// scrolls to, and what up and down move between.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Line {
    /// The row index of the header.
    Header(usize),
    /// The row indices of the items on this line.
    Items(Range<usize>),
}

/// The rows a list shows for a query, and the selection moving over them.
///
/// When the list filters, items that do not match are dropped and each
/// section is ordered by score; a section left empty loses its header too.
#[derive(Debug, Default)]
pub struct Rows {
    rows: Vec<Row>,
    lines: Vec<Line>,
    columns: Option<usize>,
}

impl Rows {
    pub fn new(list: &ListModel, query: &str) -> Self {
        let filter = list.is_filtering() && !query.trim().is_empty();
        let columns = match list.layout() {
            Layout::List => None,
            Layout::Grid { columns } => Some(usize::from(columns.max(1))),
        };
        let mut rows = Vec::new();
        let mut lines = Vec::new();
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
                lines.push(Line::Header(rows.len()));
                rows.push(Row::Header {
                    title: title.clone(),
                    subtitle: section.subtitle().cloned(),
                });
            }
            let start = rows.len();
            rows.extend(
                items
                    .into_iter()
                    .map(|(_, item)| Row::Item(Box::new(item.clone()))),
            );
            let per_line = columns.unwrap_or(1);
            lines.extend((start..rows.len()).step_by(per_line).map(|line_start| {
                Line::Items(line_start..(line_start + per_line).min(rows.len()))
            }));
        }
        Self {
            rows,
            lines,
            columns,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The number of items per line in a grid, `None` for a list.
    pub fn columns(&self) -> Option<usize> {
        self.columns
    }

    pub fn has_items(&self) -> bool {
        self.rows.iter().any(|row| matches!(row, Row::Item(_)))
    }

    pub fn item_count(&self) -> usize {
        self.items().count()
    }

    /// The row index of the selected item.
    ///
    /// The selection is kept by id, so it survives a page reloading its items.
    /// When the selected item is gone, or nothing was selected, the first item
    /// is selected: a launcher always has something `Enter` will do.
    pub fn selected_index(&self, selected: Option<&ItemId>) -> Option<usize> {
        selected
            .and_then(|id| self.position(id))
            .or_else(|| self.first_item())
    }

    /// The row index of an item, if it is shown.
    pub fn position(&self, id: &ItemId) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Item(item) if item.id() == id))
    }

    pub fn item(&self, index: usize) -> Option<&Item> {
        match self.rows.get(index)? {
            Row::Item(item) => Some(&**item),
            Row::Header { .. } => None,
        }
    }

    /// The line that draws a row.
    pub fn line_of(&self, row_ix: usize) -> Option<usize> {
        self.lines.iter().position(|line| match line {
            Line::Header(ix) => *ix == row_ix,
            Line::Items(range) => range.contains(&row_ix),
        })
    }

    /// How many items follow the selected one; what "near the end" is
    /// measured by.
    pub fn items_after(&self, selected: Option<&ItemId>) -> usize {
        let Some(selected_ix) = self.selected_index(selected) else {
            return 0;
        };
        self.rows[selected_ix + 1..]
            .iter()
            .filter(|row| matches!(row, Row::Item(_)))
            .count()
    }

    /// The item `delta` items away from the current selection in reading
    /// order, skipping headers and stopping at either end. In a list this is
    /// up and down; in a grid it is left and right, wrapping between lines.
    pub fn step(&self, selected: Option<&ItemId>, delta: isize) -> Option<&Item> {
        let items: Vec<&Item> = self.items().collect();
        let current = self
            .selected_index(selected)
            .and_then(|ix| self.item(ix))
            .and_then(|item| items.iter().position(|other| other.id() == item.id()))?;
        let next = current
            .saturating_add_signed(delta)
            .min(items.len().saturating_sub(1));
        items.get(next).copied()
    }

    /// The item `delta` lines up or down, keeping the column where the next
    /// line is long enough and taking its last item where it is not. Headers
    /// are skipped, and the selection stops at either end.
    ///
    /// In a list every line holds one item, so this is the same as
    /// [`Self::step`].
    pub fn step_lines(&self, selected: Option<&ItemId>, delta: isize) -> Option<&Item> {
        let selected_ix = self.selected_index(selected)?;
        let item_lines: Vec<&Range<usize>> = self
            .lines
            .iter()
            .filter_map(|line| match line {
                Line::Items(range) => Some(range),
                Line::Header(_) => None,
            })
            .collect();
        let current = item_lines
            .iter()
            .position(|range| range.contains(&selected_ix))?;
        let column = selected_ix - item_lines[current].start;
        let target = current
            .saturating_add_signed(delta)
            .min(item_lines.len().saturating_sub(1));
        let range = item_lines[target];
        self.item((range.start + column).min(range.end - 1))
    }

    fn items(&self) -> impl Iterator<Item = &Item> {
        self.rows.iter().filter_map(|row| match row {
            Row::Item(item) => Some(&**item),
            Row::Header { .. } => None,
        })
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
                    .with_subtitle("2")
                    .with_item(item("code", "Visual Studio Code"))
                    .with_item(item("term", "Terminal")),
            )
            .with_section(
                Section::new()
                    .with_title("Links")
                    .with_item(item("docs", "Documentation")),
            )
    }

    /// Two sections of emoji: five in the first, two in the second.
    fn grid() -> ListModel {
        ListModel::new()
            .with_layout(Layout::Grid { columns: 3 })
            .with_section(
                Section::new()
                    .with_title("Smileys")
                    .with_items(["a", "b", "c", "d", "e"].map(|id| item(id, id))),
            )
            .with_section(
                Section::new()
                    .with_title("Animals")
                    .with_items(["f", "g"].map(|id| item(id, id))),
            )
    }

    fn ids(rows: &Rows) -> Vec<String> {
        rows.rows()
            .iter()
            .map(|row| match row {
                Row::Header { title, .. } => format!("# {title}"),
                Row::Item(item) => item.id().as_str().to_owned(),
            })
            .collect()
    }

    fn id(value: &str) -> ItemId {
        ItemId::new(value)
    }

    #[test]
    fn test_filtering_drops_unmatched_items_and_empty_sections() {
        assert_eq!(
            ids(&Rows::new(&list(), "")),
            ["# Apps", "code", "term", "# Links", "docs"]
        );
        assert_eq!(ids(&Rows::new(&list(), "term")), ["# Apps", "term"]);
        assert!(!Rows::new(&list(), "zzz").has_items());
        assert!(matches!(
            &Rows::new(&list(), "").rows()[0],
            Row::Header { subtitle: Some(subtitle), .. } if subtitle == "2"
        ));
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
        assert_eq!(rows.selected_index(Some(&id("docs"))), Some(4));
        assert_eq!(rows.selected_index(Some(&id("gone"))), Some(1));

        // Filtering moves the item to another row; the selection follows it.
        let filtered = Rows::new(&list(), "docu");
        assert_eq!(filtered.selected_index(Some(&id("docs"))), Some(1));
    }

    #[test]
    fn test_stepping_skips_headers_and_stops_at_the_ends() {
        let rows = Rows::new(&list(), "");
        let term = id("term");
        assert_eq!(rows.step(Some(&term), 1).unwrap().id().as_str(), "docs");
        assert_eq!(rows.step(Some(&term), -1).unwrap().id().as_str(), "code");
        assert_eq!(rows.step(Some(&term), 10).unwrap().id().as_str(), "docs");
        assert_eq!(rows.step(Some(&term), -10).unwrap().id().as_str(), "code");
        assert!(Rows::new(&list(), "zzz").step(None, 1).is_none());
        // In a list, a line is an item.
        assert_eq!(
            rows.step_lines(Some(&term), 1).unwrap().id().as_str(),
            "docs"
        );
        assert_eq!(rows.items_after(Some(&term)), 1);
    }

    #[test]
    fn test_grid_lines_break_at_columns_and_sections() {
        let rows = Rows::new(&grid(), "");
        assert_eq!(rows.columns(), Some(3));
        assert_eq!(
            rows.lines(),
            [
                Line::Header(0),
                Line::Items(1..4),
                Line::Items(4..6),
                Line::Header(6),
                Line::Items(7..9),
            ]
        );
        assert_eq!(rows.line_of(5), Some(2));
        assert_eq!(rows.line_of(6), Some(3));
    }

    #[test]
    fn test_grid_navigation_moves_in_two_dimensions() {
        let rows = Rows::new(&grid(), "");
        let step_lines = |from: &str, delta| {
            rows.step_lines(Some(&id(from)), delta)
                .unwrap()
                .id()
                .as_str()
                .to_owned()
        };
        let step = |from: &str, delta| {
            rows.step(Some(&id(from)), delta)
                .unwrap()
                .id()
                .as_str()
                .to_owned()
        };

        // Down keeps the column.
        assert_eq!(step_lines("b", 1), "e");
        // A shorter next line gives its last item.
        assert_eq!(step_lines("c", 1), "e");
        // Down crosses into the next section, skipping its header.
        assert_eq!(step_lines("d", 1), "f");
        assert_eq!(step_lines("e", 1), "g");
        // Up from the next section returns to the same column.
        assert_eq!(step_lines("g", -1), "e");
        // The ends hold.
        assert_eq!(step_lines("b", -1), "b");
        assert_eq!(step_lines("g", 1), "g");
        // Left and right read in order and wrap between lines.
        assert_eq!(step("c", 1), "d");
        assert_eq!(step("d", -1), "c");
        assert_eq!(step("e", 1), "f");
    }
}
