//! Drives on Windows.
//!
//! A USB drive is ejected as "Safely Remove Hardware" does: the Plug and
//! Play manager is asked to eject the USB device holding the disk
//! (`CM_Request_Device_EjectW`). It flushes and dismounts every volume, and
//! when a program still has files open it refuses and names the program,
//! which the Shell's "Eject" verb would only show in its own dialog after
//! returning. Any other drive (an SD card in a built-in reader) has its
//! media ejected instead: the volume is locked, which fails while files are
//! open, dismounted, and the media released.

use std::{ffi::c_void, mem::size_of, thread, time::Duration};

use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Parent, CM_Request_Device_EjectW, CR_SUCCESS, DIGCF_DEVICEINTERFACE,
            DIGCF_PRESENT, PNP_VETO_TYPE, PNP_VetoOutstandingOpen, PNP_VetoPendingClose,
            PNP_VetoTypeUnknown, PNP_VetoWindowsApp, PNP_VetoWindowsService,
            SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
            SetupDiGetDeviceInterfaceDetailW,
        },
        Foundation::{BOOLEAN, CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, HWND},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
            OPEN_EXISTING,
        },
        System::{
            IO::DeviceIoControl,
            Ioctl::{
                FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GUID_DEVINTERFACE_DISK,
                IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_GET_DEVICE_NUMBER,
                IOCTL_STORAGE_MEDIA_REMOVAL, IOCTL_STORAGE_QUERY_PROPERTY, PREVENT_MEDIA_REMOVAL,
                PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
                STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
            },
        },
    },
    core::{HSTRING, PCWSTR},
};

use super::{Drive, EjectError};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
/// `STORAGE_BUS_TYPE::BusTypeUsb`.
const BUS_USB: i32 = 7;

/// An open volume or disk, closed when dropped.
struct Device(HANDLE);

impl Device {
    /// Opens `path` (`\\.\E:`); access 0 only asks about the device.
    fn open(path: &str, access: u32) -> windows::core::Result<Self> {
        unsafe {
            CreateFileW(
                &HSTRING::from(path),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                HANDLE::default(),
            )
        }
        .map(Self)
    }

    fn control<I, O>(
        &self,
        code: u32,
        input: Option<&I>,
        output: Option<&mut O>,
    ) -> windows::core::Result<()> {
        let input_size = input.map_or(0, |_| size_of::<I>() as u32);
        let output_size = output.as_ref().map_or(0, |_| size_of::<O>() as u32);
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                input.map(|input| input as *const I as *const c_void),
                input_size,
                output.map(|output| output as *mut O as *mut c_void),
                output_size,
                Some(&mut returned),
                None,
            )
        }
    }

    fn number(&self) -> Option<(u32, u32)> {
        let mut number = STORAGE_DEVICE_NUMBER::default();
        self.control::<(), _>(IOCTL_STORAGE_GET_DEVICE_NUMBER, None, Some(&mut number))
            .ok()?;
        Some((number.DeviceType, number.DeviceNumber))
    }

    /// The bus the disk is on, as a `STORAGE_BUS_TYPE`.
    fn bus(&self) -> Option<i32> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            ..Default::default()
        };
        // Room for the descriptor and the strings that follow it, aligned.
        let mut buffer = [0u64; 128];
        self.control(
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query),
            Some(&mut buffer),
        )
        .ok()?;
        let descriptor = unsafe { &*(buffer.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        Some(descriptor.BusType.0)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) }.ok();
    }
}

fn volume_path(letter: char) -> String {
    format!(r"\\.\{letter}:")
}

pub(super) fn list() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let system = std::env::var("SystemDrive")
        .ok()
        .and_then(|drive| super::windows_letter(&drive));
    super::letters(mask)
        .filter_map(|letter| read(letter, system == Some(letter)))
        .collect()
}

/// The drive at `letter`, if it is offered and has media in it.
fn read(letter: char, is_system: bool) -> Option<Drive> {
    let root = HSTRING::from(format!(r"{letter}:\"));
    let drive_type = unsafe { GetDriveTypeW(&root) };
    if drive_type != DRIVE_REMOVABLE && drive_type != DRIVE_FIXED {
        return None;
    }
    let bus = Device::open(&volume_path(letter), 0)
        .ok()
        .and_then(|device| device.bus());
    if !super::is_external(drive_type == DRIVE_REMOVABLE, bus, is_system) {
        return None;
    }
    // Fails when a card reader or drive has no media in it.
    let mut label = [0u16; 261];
    unsafe { GetVolumeInformationW(&root, Some(&mut label), None, None, None, None) }.ok()?;
    let length = label.iter().position(|c| *c == 0).unwrap_or(label.len());
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&root, Some(&mut free), Some(&mut total), None) }.ok();
    Some(Drive {
        path: format!(r"{letter}:\"),
        label: String::from_utf16_lossy(&label[..length]),
        total,
        free,
        device: format!("{letter}:"),
        volume: format!("{letter}:"),
        usb: bus == Some(BUS_USB),
    })
}

pub(super) fn eject(device: &str, volumes: &[Drive]) -> Result<(), EjectError> {
    let letter = super::windows_letter(device)
        .ok_or_else(|| EjectError::Failed(format!("{device} is not a drive letter.")))?;
    let volume = volume_path(letter);
    if volumes.iter().any(|drive| drive.usb)
        && let Some(disk) = disk_instance(&volume)
    {
        return request_eject(disk);
    }
    eject_media(&volume)
}

/// The Plug and Play device instance of the disk holding `volume`, found by
/// matching its device number among the disks present.
fn disk_instance(volume: &str) -> Option<u32> {
    let wanted = Device::open(volume, 0).ok()?.number()?;
    let devices = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_DISK),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .ok()?;
    let mut found = None;
    for index in 0.. {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        let listed = unsafe {
            SetupDiEnumDeviceInterfaces(
                devices,
                None,
                &GUID_DEVINTERFACE_DISK,
                index,
                &mut interface,
            )
        };
        if listed.is_err() {
            break;
        }
        let mut size = 0u32;
        // Fails by design, saying how large the detail is.
        let _ = unsafe {
            SetupDiGetDeviceInterfaceDetailW(devices, &interface, None, 0, Some(&mut size), None)
        };
        if size == 0 {
            continue;
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        let described = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                devices,
                &interface,
                Some(detail),
                size,
                None,
                Some(&mut info),
            )
        };
        if described.is_err() {
            continue;
        }
        let path = unsafe { PCWSTR(std::ptr::addr_of!((*detail).DevicePath).cast()).to_string() };
        let matches = path
            .ok()
            .and_then(|path| Device::open(&path, 0).ok())
            .and_then(|disk| disk.number())
            == Some(wanted);
        if matches {
            found = Some(info.DevInst);
            break;
        }
    }
    unsafe { SetupDiDestroyDeviceInfoList(devices) }.ok();
    found
}

/// Asks Plug and Play to eject the USB device holding `disk` (the disk
/// itself when it has no parent), a few times, as a first request is
/// sometimes refused while Explorer lets go of the drive.
fn request_eject(disk: u32) -> Result<(), EjectError> {
    let mut parent = 0u32;
    let target = match unsafe { CM_Get_Parent(&mut parent, disk, 0) } {
        CR_SUCCESS => parent,
        _ => disk,
    };
    let mut refusal = EjectError::InUse(None);
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(500));
        }
        let mut veto = PNP_VetoTypeUnknown;
        let mut name = [0u16; 260];
        let result =
            unsafe { CM_Request_Device_EjectW(target, Some(&mut veto), Some(&mut name), 0) };
        if result == CR_SUCCESS && veto == PNP_VetoTypeUnknown {
            return Ok(());
        }
        let length = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        refusal = refusal_reason(veto, &String::from_utf16_lossy(&name[..length]), result.0);
    }
    Err(refusal)
}

fn refusal_reason(veto: PNP_VETO_TYPE, name: &str, result: u32) -> EjectError {
    const UNKNOWN: i32 = PNP_VetoTypeUnknown.0;
    const WINDOWS_APP: i32 = PNP_VetoWindowsApp.0;
    const WINDOWS_SERVICE: i32 = PNP_VetoWindowsService.0;
    const OUTSTANDING_OPEN: i32 = PNP_VetoOutstandingOpen.0;
    const PENDING_CLOSE: i32 = PNP_VetoPendingClose.0;
    /// `CR_REMOVE_VETOED`.
    const VETOED: u32 = 0x17;
    match veto.0 {
        WINDOWS_APP | WINDOWS_SERVICE if !name.is_empty() => {
            EjectError::InUse(Some(super::program_name(name)))
        }
        WINDOWS_APP | WINDOWS_SERVICE | OUTSTANDING_OPEN | PENDING_CLOSE => EjectError::InUse(None),
        UNKNOWN if result == VETOED => EjectError::InUse(None),
        UNKNOWN => EjectError::Failed(format!(
            "Windows couldn’t eject it (configuration manager error {result})."
        )),
        veto => EjectError::Failed(format!("Windows refused to eject it (veto {veto}).")),
    }
}

/// Locks, dismounts and ejects the media in `volume`.
fn eject_media(volume: &str) -> Result<(), EjectError> {
    let failed = |error: windows::core::Error| EjectError::Failed(error.message().to_string());
    let device = Device::open(volume, GENERIC_READ.0 | GENERIC_WRITE.0).map_err(failed)?;
    let locked = (0..10).any(|attempt| {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(100));
        }
        device
            .control::<(), ()>(FSCTL_LOCK_VOLUME, None, None)
            .is_ok()
    });
    if !locked {
        return Err(EjectError::InUse(None));
    }
    device
        .control::<(), ()>(FSCTL_DISMOUNT_VOLUME, None, None)
        .map_err(failed)?;
    let allow = PREVENT_MEDIA_REMOVAL {
        PreventMediaRemoval: BOOLEAN(0),
    };
    device
        .control::<_, ()>(IOCTL_STORAGE_MEDIA_REMOVAL, Some(&allow), None)
        .ok();
    device
        .control::<(), ()>(IOCTL_STORAGE_EJECT_MEDIA, None, None)
        .map_err(failed)
}
