//! Eject Drives: the removable and external drives, with their free space,
//! ejected one at a time or all at once.
//!
//! Windows lists drive letters whose drive is removable, or fixed but on
//! USB, FireWire or a memory card bus, and ejects a USB drive as "Safely
//! Remove Hardware" does (`win32`). macOS asks `diskutil`, Linux `lsblk` and
//! `udisksctl`. Ejecting never takes the launcher's window away: a toast
//! says how it went, naming whatever keeps a drive in use.

#[cfg(target_os = "windows")]
mod win32;

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, WeakEntity, Window};

use crate::{
    format::format_bytes,
    model::{
        Accessory, Action, Confirmation, Effect, Image, Item, ItemId, ListModel, PageModel,
        PushHandler, RunHandler, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

/// How often the page looks for drives connected or removed meanwhile.
const REFRESH: Duration = Duration::from_secs(3);
/// Progress and outcome toasts replace each other.
const TOAST: &str = "drives/eject";

/// A mounted volume on a removable or external drive.
#[derive(Clone, Debug, Default, PartialEq)]
struct Drive {
    /// Where its files are: `E:\`, `/Volumes/USB`, `/media/me/USB`.
    path: String,
    /// The volume's name; may be empty.
    label: String,
    total: u64,
    free: u64,
    /// What ejecting removes, together with every volume sharing it: the
    /// drive letter (`E:`) on Windows, the whole disk (`disk4`, `/dev/sdb`)
    /// elsewhere.
    device: String,
    /// The volume itself, which Linux unmounts first (`/dev/sdb1`).
    volume: String,
    usb: bool,
}

impl Drive {
    /// `USB (E:)` on Windows, the volume's name elsewhere.
    fn title(&self) -> String {
        let letter = windows_letter(&self.path);
        let name = match (self.label.is_empty(), letter) {
            (false, _) => self.label.clone(),
            (true, Some(_)) if self.usb => "USB Drive".to_owned(),
            (true, Some(_)) => "Removable Disk".to_owned(),
            (true, None) => self
                .path
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or(&self.path)
                .to_owned(),
        };
        match letter {
            Some(letter) => format!("{name} ({letter}:)"),
            None => name,
        }
    }

    fn space(&self) -> String {
        match self.total {
            0 => String::new(),
            total => format!(
                "{} free of {}",
                format_bytes(self.free.min(total)),
                format_bytes(total)
            ),
        }
    }
}

/// The letter of a Windows drive root such as `E:\`.
fn windows_letter(path: &str) -> Option<char> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    matches!(chars.as_str(), ":" | ":\\").then(|| letter.to_ascii_uppercase())
}

/// The drive letters set in a `GetLogicalDrives` mask.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn letters(mask: u32) -> impl Iterator<Item = char> {
    (0..26u8)
        .filter(move |bit| mask & (1 << bit) != 0)
        .map(|bit| (b'A' + bit) as char)
}

/// Whether a Windows drive is offered: a removable one, or a fixed one on
/// an external bus (USB, FireWire, SD, MMC), never the system drive.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_external(removable: bool, bus: Option<i32>, is_system: bool) -> bool {
    const FIREWIRE: i32 = 4;
    const USB: i32 = 7;
    const SD: i32 = 12;
    const MMC: i32 = 13;
    !is_system && (removable || matches!(bus, Some(FIREWIRE | USB | SD | MMC)))
}

/// Why a drive was not ejected.
#[derive(Clone, Debug, PartialEq)]
enum EjectError {
    /// Something has files open on it: the program, when known.
    InUse(Option<String>),
    Failed(String),
}

impl std::fmt::Display for EjectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse(Some(program)) => write!(
                formatter,
                "It is in use by {program}. Close it and try again."
            ),
            Self::InUse(None) => formatter
                .write_str("It is in use. Close the files and programs using it and try again."),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

/// Reads why `diskutil` or `udisksctl` did not eject.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn classify(output: &str) -> EjectError {
    let lower = output.to_lowercase();
    if ["busy", "in use", "dissent"]
        .iter()
        .any(|word| lower.contains(word))
    {
        // diskutil: "… dissented by PID 812 (/Applications/Preview.app/…)".
        let program = output
            .split_once("dissented by PID")
            .and_then(|(_, rest)| rest.split_once('('))
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(path, _)| program_name(path));
        return EjectError::InUse(program);
    }
    EjectError::Failed(
        output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("The drive could not be ejected.")
            .to_owned(),
    )
}

/// `Preview` from `/Applications/Preview.app/Contents/MacOS/Preview` or
/// `C:\Windows\explorer.exe`.
fn program_name(path: &str) -> String {
    let path = path.trim();
    let app = path
        .split(['/', '\\'])
        .find_map(|part| part.strip_suffix(".app"));
    match app {
        Some(app) => app.to_owned(),
        None => path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned(),
    }
}

/// `true`, `1` or `"1"`: lsblk's flags vary by version.
fn flag(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => number.as_u64() == Some(1),
        serde_json::Value::String(text) => matches!(text.as_str(), "1" | "true"),
        _ => false,
    }
}

fn number(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::Number(number) => number.as_u64().unwrap_or(0),
        serde_json::Value::String(text) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn text(value: &serde_json::Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// Mount points of the running system, never offered even on a USB disk.
fn is_system_mount(mount: &str) -> bool {
    matches!(mount, "/" | "/usr" | "/var" | "/home" | "[SWAP]") || mount.starts_with("/boot")
}

/// The mounted volumes of external disks in
/// `lsblk -J -b -o NAME,PATH,LABEL,MOUNTPOINT,FSSIZE,FSAVAIL,RM,HOTPLUG,TRAN,TYPE`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_lsblk(json: &str) -> Vec<Drive> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    fn volumes<'a>(device: &'a serde_json::Value, into: &mut Vec<&'a serde_json::Value>) {
        into.push(device);
        for child in device["children"].as_array().into_iter().flatten() {
            volumes(child, into);
        }
    }
    let mut drives = Vec::new();
    for disk in value["blockdevices"].as_array().into_iter().flatten() {
        let usb = disk["tran"].as_str() == Some("usb");
        let external = flag(&disk["rm"]) || flag(&disk["hotplug"]) || usb;
        if disk["type"].as_str() != Some("disk") || !external {
            continue;
        }
        let device = match text(&disk["path"]) {
            path if path.is_empty() => format!("/dev/{}", text(&disk["name"])),
            path => path,
        };
        let mut found = Vec::new();
        volumes(disk, &mut found);
        for volume in found {
            let mount = text(&volume["mountpoint"]);
            if mount.is_empty() || is_system_mount(&mount) {
                continue;
            }
            drives.push(Drive {
                path: mount,
                label: text(&volume["label"]),
                total: number(&volume["fssize"]),
                free: number(&volume["fsavail"]),
                device: device.clone(),
                volume: match text(&volume["path"]) {
                    path if path.is_empty() => format!("/dev/{}", text(&volume["name"])),
                    path => path,
                },
                usb,
            });
        }
    }
    drives
}

/// The mounted volumes in `diskutil list -plist external`, each ejected
/// with its whole disk. Free space is filled in afterwards.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_diskutil(plist_bytes: &[u8]) -> Vec<Drive> {
    let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(plist_bytes)) else {
        return Vec::new();
    };
    let string = |dictionary: &plist::Dictionary, key: &str| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_string)
            .unwrap_or_default()
            .to_owned()
    };
    let mut drives = Vec::new();
    let disks = value
        .as_dictionary()
        .and_then(|root| root.get("AllDisksAndPartitions"))
        .and_then(plist::Value::as_array);
    for disk in disks
        .into_iter()
        .flatten()
        .filter_map(plist::Value::as_dictionary)
    {
        let device = string(disk, "DeviceIdentifier");
        let volumes = ["Partitions", "APFSVolumes"]
            .iter()
            .filter_map(|key| disk.get(key).and_then(plist::Value::as_array))
            .flatten()
            .filter_map(plist::Value::as_dictionary);
        for volume in std::iter::once(disk).chain(volumes) {
            let mount = string(volume, "MountPoint");
            if mount.is_empty() || device.is_empty() {
                continue;
            }
            drives.push(Drive {
                path: mount,
                label: string(volume, "VolumeName"),
                total: volume
                    .get("Size")
                    .and_then(plist::Value::as_unsigned_integer)
                    .unwrap_or(0),
                free: 0,
                device: device.clone(),
                volume: string(volume, "DeviceIdentifier"),
                usb: false,
            });
        }
    }
    drives
}

#[cfg(not(target_os = "windows"))]
mod unix {
    use std::process::Command;

    use super::{Drive, EjectError};

    fn run(program: &str, arguments: &[&str]) -> Result<String, EjectError> {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .map_err(|error| EjectError::Failed(format!("Cannot run {program}: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        match output.status.success() {
            true => Ok(stdout),
            false => Err(super::classify(&format!(
                "{}\n{stdout}",
                String::from_utf8_lossy(&output.stderr)
            ))),
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn list() -> Vec<Drive> {
        let Ok(output) = Command::new("diskutil")
            .args(["list", "-plist", "external"])
            .output()
        else {
            return Vec::new();
        };
        let mut drives = super::parse_diskutil(&output.stdout);
        let disks = sysinfo::Disks::new_with_refreshed_list();
        for drive in &mut drives {
            if let Some(disk) = disks
                .iter()
                .find(|disk| disk.mount_point() == std::path::Path::new(&drive.path))
            {
                drive.free = disk.available_space();
                drive.total = disk.total_space();
            }
        }
        drives
    }

    #[cfg(target_os = "macos")]
    pub(super) fn eject(device: &str, _: &[Drive]) -> Result<(), EjectError> {
        run("diskutil", &["eject", device]).map(|_| ())
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn list() -> Vec<Drive> {
        run(
            "lsblk",
            &[
                "-J",
                "-b",
                "-o",
                "NAME,PATH,LABEL,MOUNTPOINT,FSSIZE,FSAVAIL,RM,HOTPLUG,TRAN,TYPE",
            ],
        )
        .map(|json| super::parse_lsblk(&json))
        .unwrap_or_default()
    }

    /// Unmounts every volume of the disk, then powers it off so it can be
    /// pulled out; a disk that cannot be powered off is still safe to
    /// remove once unmounted.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn eject(device: &str, volumes: &[Drive]) -> Result<(), EjectError> {
        for volume in volumes {
            run("udisksctl", &["unmount", "-b", &volume.volume])?;
        }
        if let Err(error) = run("udisksctl", &["power-off", "-b", device]) {
            tracing::info!("unmounted {device} but cannot power it off: {error}");
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
use win32 as platform;

#[cfg(not(target_os = "windows"))]
use unix as platform;

/// Whether this platform's tools for listing and ejecting drives exist.
pub fn is_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::sources::process::is_installed("lsblk")
            && crate::sources::process::is_installed("udisksctl")
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(any(target_os = "windows", target_os = "macos"))
    }
}

/// The drives grouped by what ejecting removes, in listing order.
fn by_device(drives: &[Drive]) -> Vec<(String, Vec<Drive>)> {
    let mut order = Vec::new();
    let mut groups: BTreeMap<String, Vec<Drive>> = BTreeMap::new();
    for drive in drives {
        if !groups.contains_key(&drive.device) {
            order.push(drive.device.clone());
        }
        groups
            .entry(drive.device.clone())
            .or_default()
            .push(drive.clone());
    }
    order
        .into_iter()
        .filter_map(|device| groups.remove(&device).map(|volumes| (device, volumes)))
        .collect()
}

/// The name a toast gives a disk: its volumes' titles.
fn disk_name(volumes: &[Drive]) -> String {
    volumes
        .iter()
        .map(Drive::title)
        .collect::<Vec<_>>()
        .join(", ")
}

fn toast(style: ToastStyle, title: String) -> Toast {
    Toast::new(style, title).with_id(TOAST)
}

/// Ejects one disk off the main thread, reporting in toasts, and tells the
/// page so it drops the disk at once.
fn eject(device: String, volumes: Vec<Drive>, page: WeakEntity<DrivesPage>, cx: &mut App) {
    let name = disk_name(&volumes);
    perform(
        Effect::ShowToast(toast(ToastStyle::Progress, format!("Ejecting {name}…"))),
        cx,
    );
    cx.spawn(async move |cx| {
        let result = cx
            .background_spawn({
                let device = device.clone();
                async move { platform::eject(&device, &volumes) }
            })
            .await;
        cx.update(|cx| {
            let report = match &result {
                Ok(()) => toast(ToastStyle::Success, format!("Ejected {name}"))
                    .with_message("It can be removed safely."),
                Err(error) => toast(ToastStyle::Failure, format!("Couldn’t eject {name}"))
                    .with_message(error.to_string()),
            };
            perform(Effect::ShowToast(report), cx);
            if result.is_ok() {
                page.update(cx, |page, cx| {
                    if let Some(drives) = &mut page.drives {
                        drives.retain(|drive| drive.device != device);
                    }
                    cx.notify();
                })
                .ok();
            }
        });
    })
    .detach();
}

/// Ejects every disk, then says how it went: a HUD when all went, a toast
/// naming the first that did not.
fn eject_all(cx: &mut App) {
    perform(
        Effect::ShowToast(toast(ToastStyle::Progress, "Ejecting drives…".into())),
        cx,
    );
    cx.spawn(async move |cx| {
        let (ejected, failures) = cx
            .background_spawn(async move {
                let mut ejected = 0;
                let mut failures = Vec::new();
                for (device, volumes) in by_device(&platform::list()) {
                    match platform::eject(&device, &volumes) {
                        Ok(()) => ejected += 1,
                        Err(error) => failures.push((disk_name(&volumes), error)),
                    }
                }
                (ejected, failures)
            })
            .await;
        cx.update(|cx| match failures.first() {
            None => {
                let hud = match ejected {
                    0 => "No drives to eject".to_owned(),
                    1 => "Ejected 1 drive".to_owned(),
                    count => format!("Ejected {count} drives"),
                };
                crate::shell::platform::show_hud(hud.into(), cx);
            }
            Some((name, error)) => {
                let title = match failures.len() {
                    1 => format!("Couldn’t eject {name}"),
                    count => format!("Couldn’t eject {count} drives"),
                };
                let message = match failures.len() {
                    1 => error.to_string(),
                    _ => format!("{name}: {error}"),
                };
                perform(
                    Effect::ShowToast(toast(ToastStyle::Failure, title).with_message(message)),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// The root search commands this module offers on this platform.
pub fn commands() -> Vec<Item> {
    if !is_supported() {
        return Vec::new();
    }
    vec![
        crate::sources::system::command_item("system/eject-drives", "Eject Drives", "eject")
            .with_keyword("usb")
            .with_keyword("disk")
            .with_keyword("unmount")
            .with_keyword("safely remove")
            .with_keyword("external")
            .with_action(Action::new(
                "Eject Drives",
                Effect::Push(PushHandler::new(eject_drives_page)),
            )),
        crate::sources::system::command_item(
            "system/eject-all-drives",
            "Eject All Drives",
            "eject",
        )
        .with_keyword("usb")
        .with_keyword("disk")
        .with_keyword("unmount")
        .with_keyword("safely remove")
        .with_action(Action::new(
            "Eject All Drives",
            Effect::Confirm(
                Confirmation::new(
                    "Eject all drives?",
                    Effect::Run(RunHandler::new(|(), _, cx| eject_all(cx))),
                )
                .with_message("Every removable and external drive is ejected.")
                .with_confirm_title("Eject All"),
            ),
        )),
    ]
}

pub fn eject_drives_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                let drives = cx.background_spawn(async { platform::list() }).await;
                let alive = this
                    .update(cx, |page: &mut DrivesPage, cx| {
                        if page.drives.as_ref() != Some(&drives) {
                            page.drives = Some(drives);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    return;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        DrivesPage {
            drives: None,
            _refresh: refresh,
        }
    })))
}

struct DrivesPage {
    /// `None` until first listed.
    drives: Option<Vec<Drive>>,
    _refresh: Task<()>,
}

impl DrivesPage {
    fn item(&self, drive: &Drive, page: &WeakEntity<Self>) -> Item {
        let volumes: Vec<Drive> = self
            .drives
            .iter()
            .flatten()
            .filter(|other| other.device == drive.device)
            .cloned()
            .collect();
        let device = drive.device.clone();
        let page = page.clone();
        let item = Item::new(ItemId::new(format!("drives/{}", drive.path)), drive.title())
            .with_icon(match drive.usb {
                true => "usb",
                false => "hard-drive",
            })
            .with_keyword(drive.path.clone())
            .with_keyword(drive.device.clone())
            .with_action(
                Action::new(
                    "Eject",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        eject(device.clone(), volumes.clone(), page.clone(), cx)
                    })),
                )
                .with_image(Image::Icon("eject".into())),
            )
            .with_action(
                Action::new("Open", Effect::OpenPath(PathBuf::from(&drive.path)))
                    .with_image(Image::Icon("folder-open".into())),
            )
            .with_action(
                Action::new("Copy Path", Effect::Copy(drive.path.clone().into()))
                    .with_image(Image::Icon("copy".into()))
                    .with_shortcut("secondary-shift-c"),
            );
        let item = match windows_letter(&drive.path) {
            Some(_) => item,
            None => item.with_subtitle(drive.path.clone()),
        };
        match drive.space() {
            space if space.is_empty() => item,
            space => item.with_accessory(Accessory::text(space)),
        }
    }
}

impl Page for DrivesPage {
    fn title(&self) -> SharedString {
        "Eject Drives".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let items: Vec<Item> = self
            .drives
            .iter()
            .flatten()
            .map(|drive| self.item(drive, &page))
            .collect();
        items
            .into_iter()
            .fold(
                ListModel::new()
                    .with_placeholder("Search drives")
                    .with_loading(self.drives.is_none())
                    .with_empty_title(match self.drives {
                        Some(_) => "No drives to eject",
                        None => "Looking for drives…",
                    })
                    .with_empty_description(
                        "Removable and external drives appear here when connected.",
                    ),
                ListModel::with_item,
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(path: &str, label: &str, device: &str) -> Drive {
        Drive {
            path: path.into(),
            label: label.into(),
            device: device.into(),
            ..Drive::default()
        }
    }

    #[test]
    fn test_titles_and_space() {
        assert_eq!(drive("E:\\", "BACKUP", "E:").title(), "BACKUP (E:)");
        let usb = Drive {
            usb: true,
            ..drive("f:\\", "", "F:")
        };
        assert_eq!(usb.title(), "USB Drive (F:)");
        assert_eq!(drive("G:\\", "", "G:").title(), "Removable Disk (G:)");
        assert_eq!(drive("/media/me/USB", "", "/dev/sdb").title(), "USB");
        assert_eq!(
            drive("/Volumes/Photos", "Photos", "disk4").title(),
            "Photos"
        );

        let mut sized = drive("E:\\", "", "E:");
        assert_eq!(sized.space(), "", "an unknown size says nothing");
        sized.total = 32 * 1_073_741_824;
        sized.free = 8 * 1_073_741_824;
        assert_eq!(sized.space(), "8.0 GB free of 32.0 GB");
    }

    #[test]
    fn test_windows_drive_filtering() {
        assert_eq!(letters(0b1_0101).collect::<String>(), "ACE");
        assert_eq!(windows_letter("e:\\"), Some('E'));
        assert_eq!(windows_letter("/Volumes/E"), None);

        assert!(is_external(true, None, false), "removable");
        assert!(is_external(false, Some(7), false), "fixed on USB");
        assert!(is_external(false, Some(12), false), "fixed on SD");
        assert!(!is_external(false, Some(11), false), "fixed on SATA");
        assert!(!is_external(false, Some(17), false), "fixed on NVMe");
        assert!(!is_external(false, None, false), "fixed, bus unknown");
        assert!(!is_external(true, Some(7), true), "the system drive");
    }

    #[test]
    fn test_classifies_eject_failures() {
        assert_eq!(
            classify(
                "Unmount of disk4 failed: at least one volume could not be unmounted\n\
                 Unmount failed for disk4s1: dissented by PID 812 \
                 (/Applications/Preview.app/Contents/MacOS/Preview)"
            ),
            EjectError::InUse(Some("Preview".into()))
        );
        assert_eq!(
            classify("Error unmounting /dev/sdb1: target is busy"),
            EjectError::InUse(None)
        );
        assert_eq!(
            classify("\n  Error looking up object for device /dev/sdz\n"),
            EjectError::Failed("Error looking up object for device /dev/sdz".into())
        );
        assert_eq!(program_name("C:\\Windows\\explorer.exe"), "explorer.exe");
        assert_eq!(
            EjectError::InUse(Some("Preview".into())).to_string(),
            "It is in use by Preview. Close it and try again."
        );
    }

    #[test]
    fn test_parses_lsblk() {
        let json = r#"{"blockdevices": [
            {"name":"nvme0n1","path":"/dev/nvme0n1","label":null,"mountpoint":null,"fssize":null,
             "fsavail":null,"rm":false,"hotplug":false,"tran":"nvme","type":"disk","children":[
                {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","label":null,"mountpoint":"/boot/efi",
                 "fssize":"535805952","fsavail":"529305600","rm":false,"hotplug":false,"tran":null,"type":"part"},
                {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","label":null,"mountpoint":"/",
                 "fssize":"502468108288","fsavail":"300000000000","rm":false,"hotplug":false,"tran":null,"type":"part"}]},
            {"name":"sdb","path":"/dev/sdb","label":null,"mountpoint":null,"fssize":null,"fsavail":null,
             "rm":"1","hotplug":"1","tran":"usb","type":"disk","children":[
                {"name":"sdb1","path":"/dev/sdb1","label":"STICK","mountpoint":"/media/me/STICK",
                 "fssize":31000000000,"fsavail":1000000000,"rm":"1","hotplug":"1","tran":null,"type":"part"},
                {"name":"sdb2","path":"/dev/sdb2","label":"OTHER","mountpoint":null,
                 "fssize":null,"fsavail":null,"rm":"1","hotplug":"1","tran":null,"type":"part"}]},
            {"name":"loop0","path":"/dev/loop0","label":null,"mountpoint":"/snap/core/1","fssize":"1",
             "fsavail":"0","rm":false,"hotplug":false,"tran":null,"type":"loop"}
        ]}"#;
        assert_eq!(
            parse_lsblk(json),
            [Drive {
                path: "/media/me/STICK".into(),
                label: "STICK".into(),
                total: 31_000_000_000,
                free: 1_000_000_000,
                device: "/dev/sdb".into(),
                volume: "/dev/sdb1".into(),
                usb: true,
            }]
        );
        assert!(parse_lsblk("not json").is_empty());
    }

    #[test]
    fn test_parses_diskutil() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>AllDisksAndPartitions</key><array>
    <dict>
      <key>DeviceIdentifier</key><string>disk4</string>
      <key>Size</key><integer>32000000000</integer>
      <key>Partitions</key><array>
        <dict><key>DeviceIdentifier</key><string>disk4s1</string><key>VolumeName</key><string>EFI</string></dict>
        <dict><key>DeviceIdentifier</key><string>disk4s2</string><key>MountPoint</key><string>/Volumes/STICK</string>
          <key>VolumeName</key><string>STICK</string><key>Size</key><integer>31000000000</integer></dict>
      </array>
    </dict>
    <dict>
      <key>DeviceIdentifier</key><string>disk6</string>
      <key>APFSVolumes</key><array>
        <dict><key>DeviceIdentifier</key><string>disk6s1</string><key>MountPoint</key><string>/Volumes/Backup</string>
          <key>VolumeName</key><string>Backup</string><key>Size</key><integer>1000</integer></dict>
      </array>
    </dict>
  </array>
</dict></plist>"#;
        let drives = parse_diskutil(plist.as_bytes());
        assert_eq!(
            drives
                .iter()
                .map(|drive| (drive.title(), drive.device.as_str(), drive.total))
                .collect::<Vec<_>>(),
            [
                ("STICK".to_owned(), "disk4", 31_000_000_000),
                ("Backup".to_owned(), "disk6", 1000)
            ]
        );
    }

    #[test]
    fn test_groups_volumes_by_disk() {
        let drives = [
            drive("/media/a", "A", "/dev/sdc"),
            drive("/media/b", "B", "/dev/sdb"),
            drive("/media/c", "C", "/dev/sdc"),
        ];
        let groups = by_device(&drives);
        assert_eq!(
            groups
                .iter()
                .map(|(device, volumes)| (device.as_str(), disk_name(volumes)))
                .collect::<Vec<_>>(),
            [
                ("/dev/sdc", "A, C".to_owned()),
                ("/dev/sdb", "B".to_owned())
            ]
        );
    }

    #[test]
    #[ignore = "reads the machine's drives"]
    fn test_lists_drives() {
        for drive in platform::list() {
            println!(
                "{} · {} · {} · {:?}",
                drive.title(),
                drive.space(),
                drive.device,
                drive
            );
        }
    }
}
