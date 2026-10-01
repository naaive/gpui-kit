//! The controls inside a window, from UI Automation: buttons, lists,
//! panes, page elements, for selecting one control rather than the window.
//!
//! Every element read is a call into the window's application, and a
//! browser or IDE can expose tens of thousands, so the walk is bounded by
//! depth, by count and by time, and skips what the application reports as
//! off screen along with everything under it.

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::{
    Foundation::HWND,
    System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
    UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, UIA_BoundingRectanglePropertyId,
        UIA_IsOffscreenPropertyId,
    },
};

use super::rect;
use crate::geometry::PhysRect;

/// How deep under the window the walk goes.
const MAX_DEPTH: usize = 24;
/// Elements read per window at most.
const MAX_ELEMENTS: usize = 3000;
/// How long one window may take.
const BUDGET: Duration = Duration::from_millis(600);
/// Smaller elements are too small to aim at.
const MIN_SIDE: i32 = 8;

/// The on-screen controls of the window `hwnd`, cut to its `frame`.
pub fn controls(hwnd: u64, frame: PhysRect) -> Vec<PhysRect> {
    match walk(hwnd, frame) {
        Ok(parts) => parts,
        Err(error) => {
            tracing::debug!("no controls from a window: {error:#}");
            Vec::new()
        }
    }
}

fn walk(hwnd: u64, frame: PhysRect) -> Result<Vec<PhysRect>> {
    let started = Instant::now();
    // The calling thread may already be in an apartment; that's fine.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .ok();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }?;
    let cache = unsafe { automation.CreateCacheRequest() }?;
    unsafe {
        cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
        cache.AddProperty(UIA_IsOffscreenPropertyId)?;
    }
    let walker = unsafe { automation.ControlViewWalker() }?;
    let root = unsafe {
        automation.ElementFromHandleBuildCache(HWND(hwnd as *mut std::ffi::c_void), &cache)
    }?;

    let mut parts = Vec::new();
    let mut read = 0;
    let mut pending: Vec<(IUIAutomationElement, usize)> = vec![(root, 0)];
    while let Some((element, depth)) = pending.pop() {
        if read >= MAX_ELEMENTS || started.elapsed() > BUDGET {
            break;
        }
        read += 1;
        if depth > 0 {
            if unsafe { element.CachedIsOffscreen() }.is_ok_and(|offscreen| offscreen.as_bool()) {
                continue;
            }
            let Ok(bounds) = (unsafe { element.CachedBoundingRectangle() }) else {
                continue;
            };
            if let Some(part) = rect(bounds).intersect(&frame)
                && part.width >= MIN_SIDE
                && part.height >= MIN_SIDE
                && part != frame
                && !parts.contains(&part)
            {
                parts.push(part);
            }
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let mut child = unsafe { walker.GetFirstChildElementBuildCache(&element, &cache) }.ok();
        let mut children = Vec::new();
        while let Some(current) = child {
            child = unsafe { walker.GetNextSiblingElementBuildCache(&current, &cache) }.ok();
            children.push((current, depth + 1));
        }
        // Popped last-in first-out: reverse so siblings are read in order.
        pending.extend(children.into_iter().rev());
    }
    tracing::debug!(
        "read {read} elements, {} controls, in {:?}",
        parts.len(),
        started.elapsed()
    );
    Ok(parts)
}
