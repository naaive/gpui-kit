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

thread_local! {
    /// The first failure of a launcher node in the render being taken apart.
    static FAILURE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Records why a node could not be materialized.
///
/// GPUI Shell logs a failed node and draws a placeholder in its place, so its
/// parent sees an element carrying nothing and fails too, with a vaguer
/// sentence. Keeping the first failure lets [`take_failure`] report the cause
/// rather than that echo.
pub(super) fn record_failure(message: String) {
    FAILURE.with(|failure| {
        failure.borrow_mut().get_or_insert(message);
    });
}

/// Takes the recorded failure, leaving none for the next render.
pub(super) fn take_failure() -> Option<String> {
    FAILURE.with(|failure| failure.borrow_mut().take())
}
