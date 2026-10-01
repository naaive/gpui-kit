//! Radios as plain values: what each one is, whether it is on, and what
//! toggling it says.

/// What a radio transmits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Technology {
    WiFi,
    Bluetooth,
    MobileBroadband,
    Fm,
    Other,
}

impl Technology {
    /// Maps `Windows.Devices.Radios.RadioKind`.
    pub fn from_raw(value: i32) -> Self {
        match value {
            1 => Self::WiFi,
            2 => Self::MobileBroadband,
            3 => Self::Bluetooth,
            4 => Self::Fm,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::WiFi => "Wi-Fi",
            Self::Bluetooth => "Bluetooth",
            Self::MobileBroadband => "Cellular",
            Self::Fm => "FM Radio",
            Self::Other => "Radio",
        }
    }

    pub fn icon(self, power: Power) -> &'static str {
        match (self, power) {
            (Self::WiFi, Power::On) => "wifi",
            (Self::WiFi, _) => "wifi-off",
            (Self::Bluetooth, Power::On) => "bluetooth",
            (Self::Bluetooth, _) => "bluetooth-off",
            (Self::MobileBroadband, _) => "signal",
            (Self::Fm | Self::Other, _) => "radio",
        }
    }
}

/// Whether a radio is on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Power {
    #[default]
    Unknown,
    On,
    Off,
    /// Off by a hardware switch or a policy; software cannot turn it on.
    Disabled,
}

impl Power {
    /// Maps `Windows.Devices.Radios.RadioState`.
    pub fn from_raw(value: i32) -> Self {
        match value {
            1 => Self::On,
            2 => Self::Off,
            3 => Self::Disabled,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::On => "On",
            Self::Off => "Off",
            Self::Disabled => "Disabled",
        }
    }
}

/// One radio on the computer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RadioReading {
    pub name: String,
    pub technology: Technology,
    pub power: Power,
}

/// Why a radio could not be toggled, in words for the failure toast.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleError {
    /// The user turned off apps' access to radios.
    DeniedByUser,
    DeniedBySystem,
    NoRadio(Technology),
    /// Every radio of this technology is disabled.
    Disabled(Technology),
    Failed(String),
}

impl std::fmt::Display for ToggleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeniedByUser => formatter.write_str(
                "Allow apps to control radios in Settings › Privacy & security › Radios.",
            ),
            Self::DeniedBySystem => formatter.write_str("Windows doesn’t allow changing radios."),
            Self::NoRadio(technology) => {
                write!(
                    formatter,
                    "This computer has no {} radio.",
                    technology.label()
                )
            }
            Self::Disabled(technology) => write!(
                formatter,
                "{} is disabled. Check its hardware switch or airplane mode.",
                technology.label()
            ),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ToggleError {}

/// The state toggling the radios of `technology` sets: off when any is on,
/// on otherwise.
pub fn toggle_target(
    radios: &[RadioReading],
    technology: Technology,
) -> Result<Power, ToggleError> {
    let mut matching = radios
        .iter()
        .filter(|radio| radio.technology == technology)
        .peekable();
    if matching.peek().is_none() {
        return Err(ToggleError::NoRadio(technology));
    }
    let powers: Vec<Power> = matching.map(|radio| radio.power).collect();
    if powers.contains(&Power::On) {
        Ok(Power::Off)
    } else if powers.iter().all(|power| *power == Power::Disabled) {
        Err(ToggleError::Disabled(technology))
    } else {
        Ok(Power::On)
    }
}

/// The HUD after a toggle, such as `Wi-Fi Off`.
pub fn hud_text(technology: Technology, power: Power) -> String {
    format!("{} {}", technology.label(), power.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio(technology: Technology, power: Power) -> RadioReading {
        RadioReading {
            name: technology.label().to_owned(),
            technology,
            power,
        }
    }

    #[test]
    fn test_maps_radio_kinds_and_states() {
        assert_eq!(Technology::from_raw(1), Technology::WiFi);
        assert_eq!(Technology::from_raw(2), Technology::MobileBroadband);
        assert_eq!(Technology::from_raw(3), Technology::Bluetooth);
        assert_eq!(Technology::from_raw(4), Technology::Fm);
        assert_eq!(Technology::from_raw(99), Technology::Other);
        assert_eq!(Power::from_raw(1), Power::On);
        assert_eq!(Power::from_raw(2), Power::Off);
        assert_eq!(Power::from_raw(3), Power::Disabled);
        assert_eq!(Power::from_raw(0), Power::Unknown);
        assert_eq!(Technology::WiFi.icon(Power::Off), "wifi-off");
    }

    #[test]
    fn test_toggle_target() {
        let radios = [
            radio(Technology::WiFi, Power::On),
            radio(Technology::Bluetooth, Power::Off),
            radio(Technology::MobileBroadband, Power::Disabled),
        ];
        assert_eq!(toggle_target(&radios, Technology::WiFi), Ok(Power::Off));
        assert_eq!(toggle_target(&radios, Technology::Bluetooth), Ok(Power::On));
        assert_eq!(
            toggle_target(&radios, Technology::MobileBroadband),
            Err(ToggleError::Disabled(Technology::MobileBroadband))
        );
        assert_eq!(
            toggle_target(&radios, Technology::Fm),
            Err(ToggleError::NoRadio(Technology::Fm))
        );
        assert_eq!(hud_text(Technology::WiFi, Power::Off), "Wi-Fi Off");
        assert_eq!(
            ToggleError::NoRadio(Technology::Bluetooth).to_string(),
            "This computer has no Bluetooth radio."
        );
    }
}
