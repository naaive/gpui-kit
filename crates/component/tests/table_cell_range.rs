use gpui::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, Render, TestAppContext, Window,
    div,
};
use gpui_component::table::{Column, DataTable, TableDelegate, TableState};

struct Cells;

impl TableDelegate for Cells {
    fn columns_count(&self, _: &gpui::App) -> usize {
        4
    }

    fn rows_count(&self, _: &gpui::App) -> usize {
        10
    }

    fn column(&self, col_ix: usize, _: &gpui::App) -> Column {
        let name = format!("c{col_ix}");
        Column::new(name.clone(), name)
    }

    fn render_td(
        &mut self,
        _: usize,
        _: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

struct Host {
    table: Entity<TableState<Cells>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        DataTable::new(&self.table)
    }
}

#[gpui::test]
fn shift_with_arrows_selects_a_block_of_cells(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let (host, cx) = cx.add_window_view(|window, cx| Host {
        table: cx.new(|cx| TableState::new(Cells, window, cx).cell_selectable(true)),
    });
    let table = cx.update(|_, cx| host.read(cx).table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.set_selected_cell(2, 1, cx));
        let focus = table.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| table.read(cx).selected_cell_range()),
        Some((2..3, 1..2))
    );

    // The block grows from where it started, in either direction.
    cx.simulate_keystrokes("shift-down shift-down shift-right");
    assert_eq!(
        cx.update(|_, cx| table.read(cx).selected_cell_range()),
        Some((2..5, 1..3))
    );
    cx.simulate_keystrokes("shift-up shift-up shift-up shift-left shift-left");
    assert_eq!(
        cx.update(|_, cx| table.read(cx).selected_cell_range()),
        Some((1..3, 0..2))
    );

    // A plain arrow starts over from one cell.
    cx.simulate_keystrokes("down");
    assert_eq!(
        cx.update(|_, cx| table.read(cx).selected_cell_range()),
        Some((2..3, 0..1))
    );
}
