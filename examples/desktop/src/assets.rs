use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

// The status icons beyond the default component set.
icon_assets!(
    StatusIcons,
    [
        AppWindow,
        BatteryCharging,
        BatteryFull,
        BatteryLow,
        BatteryMedium,
        BatteryWarning,
        Network,
        Volume1,
        Volume2,
        VolumeX,
        Wifi,
        WifiOff,
    ]
);

pub struct DesktopAssets;

impl AssetSource for DesktopAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = StatusIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(StatusIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
