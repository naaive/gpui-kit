//! A shortcut drawn as one small cap per key, `Ctrl` `K`, the way the
//! footer, the rows and the action panel all show what a key does.

use gpui_kit::{
    AnyElement, App, IntoElement, Keystroke, ParentElement as _, SharedString, Styled as _,
    component::{ActiveTheme as _, Icon, h_flex, kbd::Kbd},
    div,
    prelude::FluentBuilder as _,
    px,
};

const ENTER: &str = "↵";

/// The caps for `keystroke` (such as `secondary-k`), or `None` when it does
/// not parse.
pub(crate) fn keycaps(keystroke: &str, cx: &App) -> Option<AnyElement> {
    let keystroke = Keystroke::parse(keystroke).ok()?;
    Some(caps(keys(&keystroke), cx))
}

/// Caps for a hotkey as settings store it: a keystroke (`ctrl-alt-v`) or
/// keys already spelled out (`Ctrl+Alt+V`).
pub(crate) fn hotkey_caps(hotkey: &str, cx: &App) -> AnyElement {
    match hotkey.contains('+') {
        true => caps(
            hotkey
                .split('+')
                .map(|key| SharedString::from(key.trim().to_owned()))
                .collect(),
            cx,
        ),
        false => keycaps(hotkey, cx).unwrap_or_else(|| caps(vec![hotkey.to_owned().into()], cx)),
    }
}

/// Caps for keys already spelled out.
fn caps(keys: Vec<SharedString>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .gap(px(3.))
        .children(keys.into_iter().map(|key| {
            let enter = key.as_ref() == ENTER;
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(20.))
                .h(px(20.))
                .px(px(5.))
                .rounded(px(5.))
                .bg(theme.foreground.opacity(0.07))
                .text_color(theme.muted_foreground)
                .text_size(px(11.))
                .map(|this| match enter {
                    // Not every UI font draws the return arrow; the icon is
                    // the same everywhere.
                    true => this.child(
                        Icon::empty()
                            .path("icons/corner-down-left.svg")
                            .size(px(11.))
                            .text_color(theme.muted_foreground),
                    ),
                    false => this.child(key),
                })
        }))
        .into_any_element()
}

/// The keys of a keystroke in the platform's order and spelling, one per cap.
fn keys(keystroke: &Keystroke) -> Vec<SharedString> {
    let modifiers = keystroke.modifiers;
    let mut keys: Vec<SharedString> = Vec::new();
    let mac = cfg!(target_os = "macos");
    let mut push = |on: bool, mac_name: &'static str, name: &'static str| {
        if on {
            keys.push(if mac { mac_name } else { name }.into());
        }
    };
    push(modifiers.control, "⌃", "Ctrl");
    push(modifiers.alt, "⌥", "Alt");
    push(modifiers.shift, "⇧", "Shift");
    push(modifiers.platform, "⌘", "Win");
    let key = match keystroke.key.as_str() {
        "enter" => ENTER.into(),
        "tab" => "⇥".into(),
        "escape" => "esc".into(),
        "space" => "Space".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        _ => {
            let bare = Keystroke {
                modifiers: Default::default(),
                ..keystroke.clone()
            };
            Kbd::format(&bare).into()
        }
    };
    keys.push(key);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spelled(keystroke: &str) -> Vec<SharedString> {
        keys(&Keystroke::parse(keystroke).unwrap())
    }

    #[test]
    fn test_keys_one_cap_per_key() {
        let secondary = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        };
        assert_eq!(spelled("secondary-k"), [secondary, "K"]);
        assert_eq!(spelled("enter"), ["↵"]);
        let expected = match cfg!(target_os = "macos") {
            true => ["⇧", "⌘", "C"],
            false => ["Ctrl", "Shift", "C"],
        };
        assert_eq!(spelled("secondary-shift-c"), expected);
    }
}
