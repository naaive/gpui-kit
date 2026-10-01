//! System Monitor: the computer's CPU, memory, disks, network, battery and
//! uptime, refreshed every couple of seconds while the page is open.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

use crate::{
    format::format_bytes,
    model::{
        Accessory, Action, Effect, Image, Item, ItemId, ListModel, PageModel, PushHandler, Section,
        Tone,
    },
    pages::{self, Page, PageHandle},
};

const REFRESH: Duration = Duration::from_secs(2);

/// One reading of everything shown.
#[derive(Clone, Debug, Default, PartialEq)]
struct Reading {
    cpu: f32,
    cores: usize,
    cpu_name: String,
    memory_used: u64,
    memory_total: u64,
    swap_used: u64,
    swap_total: u64,
    disks: Vec<DiskReading>,
    /// Bytes per second since the previous reading; `None` on the first.
    download: Option<u64>,
    upload: Option<u64>,
    battery: Option<Battery>,
    uptime: u64,
    os: String,
    host: String,
}

#[derive(Clone, Debug, PartialEq)]
struct DiskReading {
    name: String,
    mount: String,
    used: u64,
    total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Battery {
    percent: u8,
    charging: bool,
}

/// What is kept between readings: rates and CPU use need two samples.
struct Sampler {
    system: System,
    networks: Networks,
    disks: Disks,
    last: Option<(Instant, u64, u64)>,
}

impl Sampler {
    fn new() -> Self {
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last: None,
        }
    }

    fn read(&mut self) -> Reading {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.disks.refresh(true);
        let (received, transmitted) =
            self.networks
                .iter()
                .fold((0u64, 0u64), |(received, transmitted), (_, network)| {
                    (
                        received + network.total_received(),
                        transmitted + network.total_transmitted(),
                    )
                });
        let now = Instant::now();
        let rate = |before: u64, after: u64, since: Instant| {
            let seconds = now.duration_since(since).as_secs_f64().max(0.001);
            (after.saturating_sub(before) as f64 / seconds) as u64
        };
        let (download, upload) = match self.last {
            Some((since, before_in, before_out)) => (
                Some(rate(before_in, received, since)),
                Some(rate(before_out, transmitted, since)),
            ),
            None => (None, None),
        };
        self.last = Some((now, received, transmitted));
        let mut disks: Vec<DiskReading> = self
            .disks
            .iter()
            .filter(|disk| disk.total_space() > 0)
            .map(|disk| DiskReading {
                name: disk.name().to_string_lossy().into_owned(),
                mount: disk.mount_point().to_string_lossy().into_owned(),
                used: disk.total_space() - disk.available_space(),
                total: disk.total_space(),
            })
            .collect();
        disks.sort_by(|a, b| a.mount.cmp(&b.mount));
        disks.dedup_by(|a, b| a.mount == b.mount);
        Reading {
            cpu: self.system.global_cpu_usage(),
            cores: self.system.cpus().len(),
            cpu_name: self
                .system
                .cpus()
                .first()
                .map(|cpu| cpu.brand().trim().to_owned())
                .unwrap_or_default(),
            memory_used: self.system.used_memory(),
            memory_total: self.system.total_memory(),
            swap_used: self.system.used_swap(),
            swap_total: self.system.total_swap(),
            disks,
            download,
            upload,
            battery: battery(),
            uptime: System::uptime(),
            os: System::long_os_version().unwrap_or_default(),
            host: System::host_name().unwrap_or_default(),
        }
    }
}

#[cfg(target_os = "windows")]
fn battery() -> Option<Battery> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    // 128: no battery; 255: unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery {
        percent: status.BatteryLifePercent,
        charging: status.ACLineStatus == 1,
    })
}

#[cfg(not(target_os = "windows"))]
fn battery() -> Option<Battery> {
    None
}

fn percent(used: u64, total: u64) -> f32 {
    match total {
        0 => 0.,
        total => used as f32 / total as f32 * 100.,
    }
}

/// A usage figure as a tag, coloured as it nears full.
fn usage_tag(percent: f32) -> Accessory {
    let tone = match percent {
        90.0.. => Tone::Danger,
        75.0.. => Tone::Warning,
        _ => Tone::Neutral,
    };
    Accessory::tag(format!("{percent:.0}%"), tone)
}

fn format_rate(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => format!("{}/s", format_bytes(bytes)),
        None => "…".into(),
    }
}

fn format_uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match days {
        0 => format!("{hours}h {minutes}m"),
        days => format!("{days}d {hours}h {minutes}m"),
    }
}

pub fn system_monitor_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let sampler = Arc::new(Mutex::new(None::<Sampler>));
        let task = cx.spawn(async move |this, cx| {
            loop {
                let sampler = sampler.clone();
                let reading = cx
                    .background_spawn(async move {
                        let mut sampler = sampler.lock().ok()?;
                        Some(sampler.get_or_insert_with(Sampler::new).read())
                    })
                    .await;
                let alive = this
                    .update(cx, |page: &mut MonitorPage, cx| {
                        page.reading = reading;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        MonitorPage {
            reading: None,
            _task: task,
        }
    })))
}

struct MonitorPage {
    reading: Option<Reading>,
    _task: Task<()>,
}

fn row(id: &str, title: impl Into<SharedString>, icon: &str) -> Item {
    Item::new(ItemId::new(format!("monitor/{id}")), title).with_image(Image::Icon(icon.into()))
}

impl Page for MonitorPage {
    fn title(&self) -> SharedString {
        "System Monitor".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let list = ListModel::new()
            .with_placeholder("Search system information…")
            .with_loading(self.reading.is_none())
            .with_empty_title("Reading the system…");
        let Some(reading) = &self.reading else {
            return list.into();
        };
        let processes = Action::new(
            "Search Processes",
            Effect::Push(PushHandler::new(crate::processes::search_processes_page)),
        )
        .with_image(Image::Icon("cpu".into()));
        let memory = percent(reading.memory_used, reading.memory_total);
        let mut resources = vec![
            row("cpu", "CPU", "cpu")
                .with_keyword(reading.cpu_name.clone())
                .with_accessory(Accessory::text(format!("{} cores", reading.cores)))
                .with_accessory(usage_tag(reading.cpu))
                .with_action(processes.clone()),
            row("memory", "Memory", "memory-stick")
                .with_accessory(Accessory::text(format!(
                    "{} of {}",
                    format_bytes(reading.memory_used),
                    format_bytes(reading.memory_total)
                )))
                .with_accessory(usage_tag(memory))
                .with_action(processes),
        ];
        if reading.swap_total > 0 {
            let swap = percent(reading.swap_used, reading.swap_total);
            resources.push(
                row("swap", "Swap", "layers")
                    .with_accessory(Accessory::text(format!(
                        "{} of {}",
                        format_bytes(reading.swap_used),
                        format_bytes(reading.swap_total)
                    )))
                    .with_accessory(usage_tag(swap)),
            );
        }
        let disks = reading.disks.iter().enumerate().map(|(ix, disk)| {
            let used = percent(disk.used, disk.total);
            let title = match disk.name.is_empty() {
                true => disk.mount.clone(),
                false => format!("{} ({})", disk.mount, disk.name),
            };
            row(&format!("disk/{ix}"), title, "hard-drive")
                .with_accessory(Accessory::text(format!(
                    "{} free of {}",
                    format_bytes(disk.total - disk.used),
                    format_bytes(disk.total)
                )))
                .with_accessory(usage_tag(used))
                .with_action(Action::new(
                    "Open Disk",
                    Effect::OpenPath(std::path::PathBuf::from(&disk.mount)),
                ))
        });
        let network = [
            row("download", "Download", "arrow-down")
                .with_accessory(Accessory::text(format_rate(reading.download))),
            row("upload", "Upload", "arrow-up")
                .with_accessory(Accessory::text(format_rate(reading.upload))),
        ];
        let mut system = Vec::new();
        if let Some(battery) = reading.battery {
            system.push(
                row(
                    "battery",
                    match battery.charging {
                        true => "Battery · Charging",
                        false => "Battery",
                    },
                    match battery.charging {
                        true => "battery-charging",
                        false => "battery",
                    },
                )
                .with_accessory(usage_tag(battery.percent as f32)),
            );
        }
        system.extend([
            row("uptime", "Uptime", "clock")
                .with_accessory(Accessory::text(format_uptime(reading.uptime))),
            row("os", "Operating System", "monitor")
                .with_accessory(Accessory::text(reading.os.clone()))
                .with_action(Action::new("Copy", Effect::Copy(reading.os.clone().into()))),
            row("host", "Computer Name", "laptop")
                .with_accessory(Accessory::text(reading.host.clone()))
                .with_action(Action::new(
                    "Copy",
                    Effect::Copy(reading.host.clone().into()),
                )),
            row("processor", "Processor", "microchip")
                .with_accessory(Accessory::text(reading.cpu_name.clone()))
                .with_action(Action::new(
                    "Copy",
                    Effect::Copy(reading.cpu_name.clone().into()),
                )),
        ]);
        list.with_section(Section::new().with_title("Usage").with_items(resources))
            .with_section(Section::new().with_title("Disks").with_items(disks))
            .with_section(Section::new().with_title("Network").with_items(network))
            .with_section(Section::new().with_title("System").with_items(system))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_formats_figures() {
        assert_eq!(format_uptime(90_061), "1d 1h 1m");
        assert_eq!(format_uptime(3_660), "1h 1m");
        assert_eq!(percent(1, 0), 0.);
        assert_eq!(usage_tag(95.).tone(), Some(Tone::Danger));
    }

    #[test]
    fn test_reads_the_system() {
        let mut sampler = Sampler::new();
        let first = sampler.read();
        assert!(first.memory_total > 0);
        assert!(first.cores > 0);
        assert_eq!(first.download, None);
        assert!(sampler.read().download.is_some());
    }
}
