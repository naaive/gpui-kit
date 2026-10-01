//! Radios on Windows (`Windows.Devices.Radios`) and the connected Wi-Fi
//! network (`netsh`). Every function blocks: call them on a background
//! thread.

use std::process::Command;

use anyhow::{Result, anyhow};
use windows::{
    Devices::Radios::{Radio, RadioAccessStatus, RadioState},
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
};

use super::{
    netsh::{WlanInterface, parse_interfaces},
    radio::{Power, RadioReading, Technology, ToggleError, toggle_target},
};

fn initialize() {
    // Harmless when the thread is already initialized.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

fn all_radios() -> windows::core::Result<Vec<Radio>> {
    initialize();
    Ok(Radio::GetRadiosAsync()?.get()?.into_iter().collect())
}

fn reading(radio: &Radio) -> RadioReading {
    RadioReading {
        name: radio
            .Name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        technology: radio
            .Kind()
            .map(|kind| Technology::from_raw(kind.0))
            .unwrap_or(Technology::Other),
        power: radio
            .State()
            .map(|state| Power::from_raw(state.0))
            .unwrap_or_default(),
    }
}

/// Every radio on the computer.
pub fn radios() -> Result<Vec<RadioReading>> {
    Ok(all_radios()?.iter().map(reading).collect())
}

/// Turns the radios of `technology` off when any is on, else on; returns
/// the state set.
pub fn toggle(technology: Technology) -> Result<Power, ToggleError> {
    let failed = |error: windows::core::Error| ToggleError::Failed(error.message());
    initialize();
    let access = Radio::RequestAccessAsync()
        .and_then(|operation| operation.get())
        .map_err(failed)?;
    check_access(access)?;
    let radios = all_radios().map_err(failed)?;
    let readings: Vec<RadioReading> = radios.iter().map(reading).collect();
    let target = toggle_target(&readings, technology)?;
    let state = match target {
        Power::On => RadioState::On,
        _ => RadioState::Off,
    };
    for (radio, reading) in radios.iter().zip(&readings) {
        if reading.technology != technology || reading.power == Power::Disabled {
            continue;
        }
        let access = radio
            .SetStateAsync(state)
            .and_then(|operation| operation.get())
            .map_err(failed)?;
        check_access(access)?;
    }
    Ok(target)
}

fn check_access(access: RadioAccessStatus) -> Result<(), ToggleError> {
    match access {
        RadioAccessStatus::Allowed => Ok(()),
        RadioAccessStatus::DeniedByUser => Err(ToggleError::DeniedByUser),
        RadioAccessStatus::DeniedBySystem => Err(ToggleError::DeniedBySystem),
        _ => Err(ToggleError::Failed(
            "Windows didn’t say whether the radio may be changed.".into(),
        )),
    }
}

/// The wireless interfaces and the networks they are connected to.
pub fn wlan_interfaces() -> Result<Vec<WlanInterface>> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new("netsh")
        .args(["wlan", "show", "interfaces"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| anyhow!("cannot run netsh: {error}"))?;
    Ok(parse_interfaces(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    /// Lists the radios and wireless interfaces without changing any.
    #[test]
    #[ignore = "reads the machine's radios"]
    fn test_lists_radios() {
        for radio in super::radios().unwrap() {
            println!("{radio:?}");
        }
        for interface in super::wlan_interfaces().unwrap() {
            println!("{interface:?}");
        }
    }
}
