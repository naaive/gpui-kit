use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString, assets::Assets};

// The icons DataKit names itself, on top of the ones the components use.
gpui_kit::assets::icon_assets!(
    DataKitIcons,
    [
        ArrowRight,
        Braces,
        Check,
        ChevronDown,
        ChevronRight,
        CircleCheck,
        CircleStop,
        CircleX,
        Columns3,
        Copy,
        Database,
        DatabaseZap,
        Diff,
        Download,
        Eye,
        FileCode,
        Folder,
        FolderOpen,
        GitCompare,
        Hash,
        Key,
        KeyRound,
        Layers,
        Link,
        ListClock,
        ListFilter,
        ListOrdered,
        LoaderCircle,
        Minus,
        PanelRight,
        Play,
        Plug,
        Plus,
        RefreshCw,
        Search,
        Server,
        SquareFunction,
        SquareTerminal,
        Table,
        TriangleAlert,
        Undo2,
        Unplug,
        WandSparkles,
        Workflow,
        Zap,
    ]
);

/// The component icons plus DataKit's own.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match DataKitIcons.load(path)? {
            Some(data) => Ok(Some(data)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(DataKitIcons.list(path)?);
        Ok(paths)
    }
}
