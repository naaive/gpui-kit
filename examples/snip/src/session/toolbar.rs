//! The toolbar under a settled selection: drawing tools, history and what
//! to do with the capture, with a style row for the tool in hand.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _,
    button::{Button, ButtonVariants as _},
    separator::Separator,
    toolbar::{Toolbar, ToolbarGroup},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Bounds, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Size, Styled as _, div, prelude::FluentBuilder as _, px,
};

use super::{CONTEXT, CaptureSession, tool_action};
use crate::scene::{FONT_SIZES, PALETTE, STROKE_WIDTHS, Style, Tool};

/// The toolbar's width, for placing it before it has been laid out: fifteen
/// small buttons, two separators and padding.
const ESTIMATED_WIDTH: f32 = 470.;
const ROW_HEIGHT: f32 = 36.;
const GAP: f32 = 8.;

fn icon(tool: Tool) -> IconName {
    match tool {
        Tool::Rectangle => IconName::Square,
        Tool::Ellipse => IconName::Circle,
        Tool::Arrow => IconName::MoveUpRight,
        Tool::Line => IconName::Slash,
        Tool::Pen => IconName::Pencil,
        Tool::Marker => IconName::Highlighter,
        Tool::Mosaic => IconName::Grid3x3,
        Tool::Text => IconName::Type,
        Tool::Step => IconName::ListOrdered,
    }
}

/// The toolbar for `session`, placed against `selection` (logical) inside a
/// display of `display_size`.
pub fn render(
    session: &CaptureSession,
    selection: Bounds<Pixels>,
    display_size: Size<Pixels>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let state = session.state();
    let tool = state.tool();
    let history = state.history();
    let rows = if tool.is_some() { 2. } else { 1. };
    let height = px(ROW_HEIGHT * rows + GAP * (rows - 1.));
    let gap = px(GAP);

    let below = selection.bottom() + gap + height <= display_size.height;
    let above = selection.top() - gap - height >= px(0.);
    let fits_right = selection.right() - px(ESTIMATED_WIDTH) >= px(0.);

    let tools = ToolbarGroup::new("tools")
        .label("Tools")
        .gap_0p5()
        .children(Tool::ALL.into_iter().map(|candidate| {
            let (action, _) = tool_action(candidate);
            let dispatched = action.boxed_clone();
            Button::new(ElementId::Name(candidate.title().into()))
                .icon(icon(candidate))
                .selected(tool == Some(candidate))
                .tooltip_with_action(candidate.title(), &*action, Some(CONTEXT))
                .on_click(move |_, window, cx| window.dispatch_action(dispatched.boxed_clone(), cx))
        }));
    let history_group = ToolbarGroup::new("history")
        .label("History")
        .gap_0p5()
        .child(command_button(
            "undo",
            IconName::Undo2,
            "Undo",
            Box::new(super::Undo),
            !history.is_undoable(),
        ))
        .child(command_button(
            "redo",
            IconName::Redo2,
            "Redo",
            Box::new(super::Redo),
            !history.is_redoable(),
        ));
    let output = ToolbarGroup::new("output")
        .label("Output")
        .gap_0p5()
        .child(command_button(
            "pin",
            IconName::Pin,
            "Pin to screen",
            Box::new(super::Pin),
            false,
        ))
        .child(command_button(
            "save",
            IconName::Download,
            "Save",
            Box::new(super::Save),
            false,
        ))
        .child(command_button(
            "save-as",
            IconName::Save,
            "Save as…",
            Box::new(super::SaveAs),
            false,
        ))
        .child(command_button(
            "cancel",
            IconName::X,
            "Cancel",
            Box::new(super::Cancel),
            false,
        ))
        .child(
            Button::new("copy")
                .icon(IconName::Check)
                .primary()
                .tooltip_with_action("Copy", &super::Copy, Some(CONTEXT))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(super::Copy), cx)),
        );

    let main_row = Toolbar::new("capture-toolbar")
        .gap_1()
        .p_1()
        .child(tools)
        .content(Separator::vertical().h_5())
        .child(history_group)
        .content(Separator::vertical().h_5())
        .child(output);

    v_flex()
        .id("toolbar")
        .absolute()
        .occlude()
        .gap(gap)
        .items_end()
        .when(fits_right, |this| {
            this.right(display_size.width - selection.right())
        })
        .when(!fits_right, |this| this.left(px(4.)))
        .map(|this| {
            if below {
                this.top(selection.bottom() + gap)
            } else if above {
                this.top(selection.top() - gap - height)
            } else {
                this.top(selection.bottom() - gap - height)
            }
        })
        .child(surface(main_row, cx))
        .when_some(tool, |this, tool| {
            this.child(surface(style_row(tool, state.style(), cx), cx))
        })
        .text_color(theme.popover_foreground)
        .into_any_element()
}

/// A toolbar row on the popover surface.
fn surface(content: impl IntoElement, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_md()
        .child(content)
}

fn command_button(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    action: Box<dyn gpui_kit::Action>,
    is_disabled: bool,
) -> Button {
    let dispatched = action.boxed_clone();
    Button::new(id)
        .icon(icon)
        .disabled(is_disabled)
        .tooltip_with_action(title, &*action, Some(CONTEXT))
        .on_click(move |_, window, cx| window.dispatch_action(dispatched.boxed_clone(), cx))
}

/// Color, size and fill for the tool in hand.
fn style_row(tool: Tool, style: &Style, cx: &App) -> impl IntoElement {
    let style = *style;
    let ink = cx.theme().foreground;
    let colors = ToolbarGroup::new("colors")
        .label("Color")
        .gap_0p5()
        .children(PALETTE.into_iter().enumerate().map(move |(ix, color)| {
            Button::new(("color", ix))
                .selected(style.color() == color)
                .tooltip(color.hex())
                .accessibility_label(color.hex())
                .child(
                    div()
                        .size_4()
                        .rounded_full()
                        .border_1()
                        .border_color(super::paint::hsla(color.contrasting().with_alpha(64)))
                        .bg(super::paint::hsla(color)),
                )
                .on_click(move |_, window, cx| {
                    set_style(style.with_color(color), window, cx);
                })
        }));
    let sizes = if tool.has_stroke() {
        ToolbarGroup::new("stroke")
            .label("Stroke width")
            .gap_0p5()
            .children(
                STROKE_WIDTHS
                    .into_iter()
                    .enumerate()
                    .map(move |(ix, width)| {
                        Button::new(("stroke", ix))
                            .selected(style.stroke_width() == width)
                            .tooltip(format!("{width} px"))
                            .accessibility_label(format!("{width} pixels"))
                            .child(div().size(px(width + 2.)).rounded_full().bg(ink))
                            .on_click(move |_, window, cx| {
                                set_style(style.with_stroke_width(width), window, cx);
                            })
                    }),
            )
    } else {
        ToolbarGroup::new("font-size")
            .label("Font size")
            .gap_0p5()
            .children(FONT_SIZES.into_iter().enumerate().map(move |(ix, size)| {
                Button::new(("font-size", ix))
                    .selected(style.font_size() == size)
                    .label(["S", "M", "L"][ix])
                    .tooltip(format!("{size} pt"))
                    .on_click(move |_, window, cx| {
                        set_style(style.with_font_size(size), window, cx);
                    })
            }))
    };
    Toolbar::new("style-toolbar")
        .gap_1()
        .p_1()
        .child(colors)
        .content(Separator::vertical().h_5())
        .child(sizes)
        .when(tool.can_fill(), |this| {
            this.content(Separator::vertical().h_5()).child(
                ToolbarGroup::new("fill").label("Fill").child(
                    Button::new("fill")
                        .icon(IconName::PaintBucket)
                        .selected(style.is_filled())
                        .tooltip("Fill")
                        .on_click(move |_, window, cx| {
                            set_style(style.with_filled(!style.is_filled()), window, cx);
                        }),
                ),
            )
        })
}

/// Applies `style` to the open session.
fn set_style(style: Style, window: &mut gpui_kit::Window, cx: &mut App) {
    if let Some(session) = crate::app::session(cx) {
        session.update(cx, |session, cx| {
            session.update_state(window, cx, |state, _| {
                state.set_style(style);
                super::machine::Effect::None
            })
        });
    }
}
