//! Handing a model value from a materializer to whoever asked for it.
//!
//! GPUI Shell materializes every node into an erased `AnyElement`. The
//! launcher's nodes draw nothing, so the element is only an envelope: a
//! [`Carrier`] holds the value, and [`take`] is the only way back out. This is
//! the same pattern `gpui-component-shell` uses for typed children.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, div,
};

pub(super) struct Carrier<T: 'static>(Option<T>);

impl<T: 'static> Carrier<T> {
    pub(super) fn new(value: T) -> Self {
        Self(Some(value))
    }
}

impl<T: 'static> IntoElement for Carrier<T> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<T: 'static> Element for Carrier<T> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut element = div().into_any_element();
        let id = element.request_layout(window, cx);
        (id, element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

/// Takes the value an element carries, or `None` if it carries something else.
pub(super) fn take<T: 'static>(element: &mut AnyElement) -> Option<T> {
    element.downcast_mut::<Carrier<T>>()?.0.take()
}
