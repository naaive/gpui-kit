//! X11: the root window's pixels and its window stack.
//!
//! GPUI treats an X screen as one display spanning every monitor, so the
//! whole root window is one frame, and a selection may cross monitors. Its
//! scale is the one GPUI uses: `GPUI_X11_SCALE_FACTOR`, else `Xft.dpi`.

use anyhow::{Context as _, Result, bail};
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, Window},
    rust_connection::RustConnection,
};

use super::super::{Frame, bgra_to_rgba};
use crate::geometry::{DisplayArea, PhysPoint, PhysRect, WindowSnapshot};

fn connect() -> Result<(RustConnection, Window)> {
    let (connection, screen_ix) = x11rb::connect(None).context("cannot reach the X server")?;
    let root = connection
        .setup()
        .roots
        .get(screen_ix)
        .context("the X server has no such screen")?
        .root;
    Ok((connection, root))
}

pub fn capture() -> Result<Vec<Frame>> {
    let (connection, root) = connect()?;
    let geometry = connection.get_geometry(root)?.reply()?;
    let (width, height) = (geometry.width, geometry.height);
    let image = connection
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, width, height, !0)?
        .reply()
        .context("the X server refused to copy the screen")?;
    if !matches!(image.depth, 24 | 32) {
        bail!("unsupported screen depth {}", image.depth);
    }
    let (width, height) = (width as usize, height as usize);
    let stride = image.data.len() / height.max(1);
    if stride < width * 4 {
        bail!("unexpected screen image layout");
    }
    // A 24- or 32-bit ZPixmap on a little-endian server is BGRX.
    let pixels = bgra_to_rgba(&image.data, width, height, stride);
    let area = DisplayArea::new(
        PhysRect::new(0, 0, width as i32, height as i32),
        scale_factor(&connection, root),
    );
    Ok(vec![Frame::new(area, 0, pixels)?])
}

/// The interface scale GPUI applies on this X screen.
fn scale_factor(connection: &RustConnection, root: Window) -> f32 {
    if let Some(scale) = std::env::var("GPUI_X11_SCALE_FACTOR")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|scale| scale.is_normal() && *scale > 0.)
    {
        return scale;
    }
    resource_dpi(connection, root).map_or(1., |dpi| dpi / 96.)
}

/// `Xft.dpi` from the root window's resource database.
fn resource_dpi(connection: &RustConnection, root: Window) -> Option<f32> {
    let reply = connection
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            u32::MAX / 4,
        )
        .ok()?
        .reply()
        .ok()?;
    parse_dpi(&String::from_utf8_lossy(&reply.value))
}

fn parse_dpi(resources: &str) -> Option<f32> {
    resources.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == "Xft.dpi")
            .then(|| value.trim().parse::<f32>().ok())
            .flatten()
            .filter(|dpi| *dpi > 0.)
    })
}

pub fn pointer() -> Result<PhysPoint> {
    let (connection, root) = connect()?;
    let reply = connection.query_pointer(root)?.reply()?;
    Ok(PhysPoint::new(reply.root_x as i32, reply.root_y as i32))
}

/// The managed windows that are shown, frontmost first, as their frames
/// with decorations and without client-side shadows.
pub fn window_snapshots() -> Result<Vec<WindowSnapshot>> {
    let (connection, root) = connect()?;
    let atom = |name: &str| -> Result<u32> {
        Ok(connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    };
    let stacking = atom("_NET_CLIENT_LIST_STACKING")?;
    let frame_extents = atom("_NET_FRAME_EXTENTS")?;
    let gtk_frame_extents = atom("_GTK_FRAME_EXTENTS")?;
    let state = atom("_NET_WM_STATE")?;
    let hidden = atom("_NET_WM_STATE_HIDDEN")?;

    let list = connection
        .get_property(false, root, stacking, AtomEnum::WINDOW, 0, u32::MAX / 4)?
        .reply()?;
    let windows: Vec<Window> = list
        .value32()
        .context("the window manager doesn't list its windows")?
        .collect();
    let cardinals = |window: Window, property: u32| -> Vec<u32> {
        connection
            .get_property(false, window, property, AtomEnum::ANY, 0, 64)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(|values| values.collect()))
            .unwrap_or_default()
    };

    // The list is bottom to top.
    let mut snapshots = Vec::new();
    for window in windows.into_iter().rev() {
        if cardinals(window, state).contains(&hidden) {
            continue;
        }
        let Ok(Ok(geometry)) = connection.get_geometry(window).map(|cookie| cookie.reply()) else {
            continue;
        };
        let Ok(Ok(origin)) = connection
            .translate_coordinates(window, root, 0, 0)
            .map(|cookie| cookie.reply())
        else {
            continue;
        };
        let content = PhysRect::new(
            origin.dst_x as i32,
            origin.dst_y as i32,
            geometry.width as i32,
            geometry.height as i32,
        );
        let frame = frame_rect(
            content,
            &cardinals(window, frame_extents),
            &cardinals(window, gtk_frame_extents),
        );
        if !frame.is_empty() {
            snapshots.push(WindowSnapshot::new(frame));
        }
    }
    Ok(snapshots)
}

/// A window's visible frame: its content grown by the window manager's
/// decorations (`_NET_FRAME_EXTENTS`) and shrunk by the shadow a client
/// draws itself (`_GTK_FRAME_EXTENTS`). Both are left, right, top, bottom.
fn frame_rect(content: PhysRect, decorations: &[u32], shadows: &[u32]) -> PhysRect {
    let edge = |values: &[u32], ix: usize| values.get(ix).copied().unwrap_or(0) as i32;
    let left = content.x - edge(decorations, 0) + edge(shadows, 0);
    let right = content.right() + edge(decorations, 1) - edge(shadows, 1);
    let top = content.y - edge(decorations, 2) + edge(shadows, 2);
    let bottom = content.bottom() + edge(decorations, 3) - edge(shadows, 3);
    PhysRect::from_edges(left, top, right, bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dpi() {
        assert_eq!(parse_dpi("Xft.antialias:\t1\nXft.dpi:\t144\n"), Some(144.));
        assert_eq!(parse_dpi("Xcursor.size: 24"), None);
        assert_eq!(parse_dpi("Xft.dpi: 0"), None);
    }

    #[test]
    fn test_frame_rect() {
        let content = PhysRect::new(100, 100, 400, 300);
        assert_eq!(
            frame_rect(content, &[2, 2, 30, 2], &[]),
            PhysRect::new(98, 70, 404, 332),
            "decorations are part of the frame"
        );
        assert_eq!(
            frame_rect(content, &[], &[20, 20, 10, 30]),
            PhysRect::new(120, 110, 360, 260),
            "client-side shadows are not"
        );
    }
}
