//! Toasts: asynchronous status that needs no decision.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    App, Window,
    component::{
        Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        notification::{Notification, NotificationType},
    },
    prelude::FluentBuilder as _,
};

use crate::model::{Toast, ToastStyle};

/// Keys toasts that carry an id, so a later toast with the same id replaces
/// the earlier one in place instead of stacking beside it.
struct LauncherToast;

pub(super) fn show_toast(toast: &Toast, window: &mut Window, cx: &mut App) {
    let notification_type = match toast.style() {
        ToastStyle::Info | ToastStyle::Progress => NotificationType::Info,
        ToastStyle::Success => NotificationType::Success,
        ToastStyle::Failure => NotificationType::Error,
    };
    let notification = Notification::new()
        .title(toast.title().clone())
        .with_type(notification_type)
        .when_some(toast.message().cloned(), Notification::message)
        .when_some(toast.id().cloned(), |this, id| {
            this.id1::<LauncherToast>(id)
        })
        // Work in progress stays until the toast that reports its outcome
        // replaces it; hiding it on a timer would claim the work had ended.
        .when(toast.style() == ToastStyle::Progress, |this| {
            this.autohide(false)
        });
    // The button and the dismissal each report once, and pressing the button
    // closes the toast without also reporting a dismissal.
    let pressed = Rc::new(Cell::new(false));
    let notification = match toast.action().cloned() {
        Some((title, handler)) => {
            let pressed = pressed.clone();
            notification.action(move |_, _, _| {
                let (handler, pressed) = (handler.clone(), pressed.clone());
                Button::new("toast-action")
                    .small()
                    .primary()
                    .label(title.clone())
                    .on_click(move |_, window, cx| {
                        if !pressed.replace(true) {
                            handler.run(window, cx);
                        }
                    })
            })
        }
        None => notification,
    };
    let notification = match toast.on_dismiss().cloned() {
        Some(handler) => notification.on_close(move |window, cx| {
            if !pressed.replace(true) {
                handler.run(window, cx);
            }
        }),
        None => notification,
    };
    window.push_notification(notification, cx);
}
