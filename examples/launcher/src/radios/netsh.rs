//! Reading `netsh wlan show interfaces`, which names the Wi-Fi network each
//! wireless interface is connected to.
//!
//! The labels are localized with Windows; on a system in another language
//! nothing is recognized and no network name is shown.

/// One wireless interface as `netsh` reports it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WlanInterface {
    pub name: String,
    pub description: String,
    /// Such as `connected` or `disconnected`.
    pub state: String,
    /// The network's name, while connected.
    pub ssid: Option<String>,
    /// Signal quality in percent.
    pub signal: Option<u8>,
    /// Such as `5 GHz`.
    pub band: Option<String>,
    /// Such as `802.11ax`.
    pub radio_type: Option<String>,
    pub authentication: Option<String>,
    /// Receive rate in Mbps.
    pub receive_rate: Option<String>,
}

impl WlanInterface {
    pub fn is_connected(&self) -> bool {
        self.state.eq_ignore_ascii_case("connected")
    }
}

/// The interfaces in `netsh wlan show interfaces` output, in order.
pub fn parse_interfaces(output: &str) -> Vec<WlanInterface> {
    let mut interfaces: Vec<WlanInterface> = Vec::new();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "Name" {
            interfaces.push(WlanInterface {
                name: value.to_owned(),
                ..WlanInterface::default()
            });
            continue;
        }
        let Some(interface) = interfaces.last_mut() else {
            continue;
        };
        let text = || (!value.is_empty()).then(|| value.to_owned());
        match key {
            "Description" => interface.description = value.to_owned(),
            "State" => interface.state = value.to_owned(),
            "SSID" => interface.ssid = text(),
            "Signal" => interface.signal = value.trim_end_matches('%').trim().parse().ok(),
            "Band" => interface.band = text(),
            "Radio type" => interface.radio_type = text(),
            "Authentication" => interface.authentication = text(),
            "Receive rate (Mbps)" => interface.receive_rate = text(),
            _ => {}
        }
    }
    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONNECTED: &str = "
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Intel(R) Wi-Fi 6 AX201 160MHz
    GUID                   : 4f8e2d1a-9c3b-4e7f-8a2d-6b1c0e9f7a35
    Physical address       : a4:c3:f0:12:34:56
    Interface type         : Primary
    State                  : connected
    SSID                   : Home Network 5G
    AP BSSID               : 3c:84:6a:aa:bb:cc
    Band                   : 5 GHz
    Channel                : 44
    Network type           : Infrastructure
    Radio type             : 802.11ax
    Authentication         : WPA2-Personal
    Cipher                 : CCMP
    Connection mode        : Auto Connect
    Receive rate (Mbps)    : 1201
    Transmit rate (Mbps)   : 1201
    Signal                 : 92%
    Profile                : Home Network 5G
    QoS MSCS Configured         : 0
    QoS Map Configured          : 0
    QoS Map Allowed by Policy   : 0

    Hosted network status  : Not available
";

    #[test]
    fn test_parses_a_connected_interface() {
        let interfaces = parse_interfaces(CONNECTED);
        assert_eq!(interfaces.len(), 1);
        let wifi = &interfaces[0];
        assert_eq!(wifi.name, "Wi-Fi");
        assert_eq!(wifi.description, "Intel(R) Wi-Fi 6 AX201 160MHz");
        assert!(wifi.is_connected());
        assert_eq!(wifi.ssid.as_deref(), Some("Home Network 5G"));
        assert_eq!(wifi.signal, Some(92));
        assert_eq!(wifi.band.as_deref(), Some("5 GHz"));
        assert_eq!(wifi.radio_type.as_deref(), Some("802.11ax"));
        assert_eq!(wifi.authentication.as_deref(), Some("WPA2-Personal"));
        assert_eq!(wifi.receive_rate.as_deref(), Some("1201"));
    }

    #[test]
    fn test_parses_disconnected_and_missing_interfaces() {
        let output = "
There are 2 interfaces on the system:

    Name                   : Wi-Fi
    Description            : Realtek RTL8852BE WiFi 6 802.11ax PCIe Adapter
    GUID                   : 0d1e2f3a-4b5c-6d7e-8f90-a1b2c3d4e5f6
    Physical address       : 10:5b:ad:01:02:03
    Interface type         : Primary
    State                  : disconnected
    Radio status           : Hardware On
                             Software Off

    Name                   : Wi-Fi 2
    Description            : TP-Link Wireless USB Adapter
    State                  : connected
    SSID                   : Café: Guest
    Signal                 : 40%

    Hosted network status  : Not available
";
        let interfaces = parse_interfaces(output);
        assert_eq!(interfaces.len(), 2);
        assert!(!interfaces[0].is_connected());
        assert_eq!(interfaces[0].ssid, None);
        assert_eq!(interfaces[0].signal, None);
        assert_eq!(
            interfaces[1].ssid.as_deref(),
            Some("Café: Guest"),
            "a colon in the name is kept"
        );
        assert_eq!(interfaces[1].signal, Some(40));
        assert!(parse_interfaces("There is no wireless interface on the system.").is_empty());
    }
}
