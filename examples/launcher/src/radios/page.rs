//! Network Status: the computer's radios and the Wi-Fi network it is
//! connected to, refreshed while the page is open.

use std::time::Duration;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, WeakEntity, Window};

use super::{
    netsh::WlanInterface,
    platform,
    radio::{Power, RadioReading, Technology},
};
use crate::{
    model::{
        Accessory, Action, Effect, Image, Item, ItemId, ListModel, PageModel, RunHandler, Section,
        Toast, ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

const REFRESH: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Default)]
struct Status {
    radios: Vec<RadioReading>,
    interfaces: Vec<WlanInterface>,
}

fn read_status() -> Result<Status, String> {
    let radios = platform::radios().map_err(|error| format!("{error:#}"))?;
    // Without `netsh` or a wireless interface the radios still show.
    let interfaces = platform::wlan_interfaces().unwrap_or_default();
    Ok(Status { radios, interfaces })
}

pub fn network_status_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(
        |cx: &mut Context<NetworkStatusPage>| {
            let task = cx.spawn(async move |this, cx| {
                loop {
                    let status = cx.background_spawn(async { read_status() }).await;
                    let alive = this
                        .update(cx, |page: &mut NetworkStatusPage, cx| {
                            page.status = Some(status);
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        break;
                    }
                    cx.background_executor().timer(REFRESH).await;
                }
            });
            NetworkStatusPage {
                status: None,
                _refresh: task,
                toggle: None,
            }
        },
    )))
}

pub struct NetworkStatusPage {
    /// `None` until the first reading.
    status: Option<Result<Status, String>>,
    _refresh: Task<()>,
    /// The toggle in flight, then the reading after it.
    toggle: Option<Task<()>>,
}

impl NetworkStatusPage {
    fn toggle(&mut self, technology: Technology, cx: &mut Context<Self>) {
        if self.toggle.is_some() {
            return;
        }
        self.toggle = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { platform::toggle(technology) })
                .await;
            let status = cx.background_spawn(async { read_status() }).await;
            this.update(cx, |page, cx| {
                if let Err(error) = result {
                    perform(
                        Effect::ShowToast(
                            Toast::new(
                                ToastStyle::Failure,
                                format!("Couldn’t change {}", technology.label()),
                            )
                            .with_message(error.to_string()),
                        ),
                        cx,
                    );
                }
                page.status = Some(status);
                page.toggle = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

fn radio_item(ix: usize, radio: &RadioReading, page: &WeakEntity<NetworkStatusPage>) -> Item {
    let tone = match radio.power {
        Power::On => Tone::Success,
        Power::Disabled => Tone::Warning,
        Power::Off | Power::Unknown => Tone::Neutral,
    };
    let title = match radio.name.trim() {
        "" => radio.technology.label().to_owned(),
        name => name.to_owned(),
    };
    let mut item = Item::new(ItemId::new(format!("network/radio/{ix}")), title)
        .with_image(Image::Icon(radio.technology.icon(radio.power).into()))
        .with_keyword(radio.technology.label())
        .with_accessory(Accessory::tag(radio.power.label(), tone));
    if matches!(radio.power, Power::On | Power::Off) {
        let page = page.clone();
        let technology = radio.technology;
        item = item.with_action(
            Action::new(
                match radio.power {
                    Power::On => "Turn Off",
                    _ => "Turn On",
                },
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    page.update(cx, |page, cx| page.toggle(technology, cx)).ok();
                })),
            )
            .with_image(Image::Icon("power".into())),
        );
    }
    let settings = match radio.technology {
        Technology::Bluetooth => Some("ms-settings:bluetooth"),
        Technology::WiFi => Some("ms-settings:network-wifi"),
        Technology::MobileBroadband => Some("ms-settings:network-cellular"),
        Technology::Fm | Technology::Other => None,
    };
    if let Some(url) = settings {
        item = item.with_action(
            Action::new("Open Settings", Effect::OpenUrl(url.into()))
                .with_image(Image::Icon("settings".into())),
        );
    }
    item
}

fn interface_item(ix: usize, interface: &WlanInterface) -> Item {
    let connected = interface.is_connected();
    let title = match (&interface.ssid, connected) {
        (Some(ssid), true) => ssid.clone(),
        _ => "Not Connected".to_owned(),
    };
    let mut item = Item::new(ItemId::new(format!("network/wlan/{ix}")), title)
        .with_image(Image::Icon(
            match connected {
                true => "wifi",
                false => "wifi-off",
            }
            .into(),
        ))
        .with_subtitle(interface.name.clone())
        .with_keyword(interface.description.clone());
    for detail in [&interface.band, &interface.radio_type]
        .into_iter()
        .flatten()
    {
        item = item.with_accessory(Accessory::text(detail.clone()));
    }
    if let Some(signal) = interface.signal.filter(|_| connected) {
        let tone = match signal {
            0..=30 => Tone::Warning,
            _ => Tone::Neutral,
        };
        item =
            item.with_accessory(Accessory::tag(format!("{signal}%"), tone).with_tooltip("Signal"));
    }
    if let (Some(ssid), true) = (&interface.ssid, connected) {
        item = item.with_action(Action::new(
            "Copy Network Name",
            Effect::Copy(ssid.clone().into()),
        ));
    }
    item.with_action(
        Action::new(
            "Open Wi-Fi Settings",
            Effect::OpenUrl("ms-settings:network-wifi".into()),
        )
        .with_image(Image::Icon("settings".into())),
    )
}

impl Page for NetworkStatusPage {
    fn title(&self) -> SharedString {
        "Network Status".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let list = ListModel::new()
            .with_placeholder("Search radios and networks…")
            .with_loading(self.status.is_none() || self.toggle.is_some());
        let status = match &self.status {
            None => return list.with_empty_title("Reading the radios…").into(),
            Some(Err(error)) => {
                return PageModel::failure("Couldn’t read the radios", error.clone());
            }
            Some(Ok(status)) => status,
        };
        let page = cx.entity().downgrade();
        list.with_empty_title("No radios")
            .with_section(
                Section::new().with_title("Wi-Fi").with_items(
                    status
                        .interfaces
                        .iter()
                        .enumerate()
                        .map(|(ix, interface)| interface_item(ix, interface)),
                ),
            )
            .with_section(
                Section::new().with_title("Radios").with_items(
                    status
                        .radios
                        .iter()
                        .enumerate()
                        .map(|(ix, radio)| radio_item(ix, radio, &page)),
                ),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
