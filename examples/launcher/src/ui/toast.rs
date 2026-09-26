//! Toasts: asynchronous status that needs no decision.

use gpui_kit::{
    App, Window,
    component::{
        WindowExt as _,
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
    window.push_notification(notification, cx);
}
