//! Keep Awake: keeps the computer from sleeping until stopped, for a while,
//! or until a time, as Raycast's Coffee does, optionally keeping the display
//! on too.
//!
//! The operating system is asked through its own mechanism: on Windows a
//! thread holds `SetThreadExecutionState` (the request belongs to the thread
//! that made it, so the thread lives until the request is withdrawn), on
//! macOS a `caffeinate` child that also ends with the launcher, and on Linux
//! a `systemd-inhibit` child. An end time is kept by a timer here, and the
//! root search's "Keep Awake" row says until when.

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveTime, TimeDelta};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task, Window};

use crate::{
    extensions::CommandId,
    model::{
        Accessory, Action, ActionPanel, Control, Effect, Field, FormHandler, FormModel, FormValue,
        Image, Item, ItemId, ListModel, PageModel, PushHandler, RunHandler, Section, Toast,
        ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

/// The root search command whose subtitle tells the state.
const COMMAND: &str = "keep-awake";
/// The longest a typed duration may be: a week.
const LONGEST_MINUTES: u32 = 7 * 24 * 60;
/// How often an end time is checked; the wall clock decides, so a timer
/// that runs late never keeps the computer awake much longer.
const CHECK: Duration = Duration::from_secs(30);
/// How often the open page redraws its time left.
const REDRAW: Duration = Duration::from_secs(15);

/// The lengths the page offers: minutes, or `None` until stopped.
const LENGTHS: [(Option<u32>, &str); 6] = [
    (None, "Until Stopped"),
    (Some(15), "15 Minutes"),
    (Some(30), "30 Minutes"),
    (Some(60), "1 Hour"),
    (Some(120), "2 Hours"),
    (Some(240), "4 Hours"),
];

/// One period of keeping awake.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Session {
    /// `None` until stopped.
    ends_at: Option<DateTime<Local>>,
    /// Whether the display is kept on too.
    display: bool,
}

impl Session {
    fn remaining(&self, now: DateTime<Local>) -> Option<TimeDelta> {
        self.ends_at
            .map(|ends_at| (ends_at - now).max(TimeDelta::zero()))
    }

    fn is_over(&self, now: DateTime<Local>) -> bool {
        self.ends_at.is_some_and(|ends_at| ends_at <= now)
    }
}

/// Which session runs, apart from the operating system, so tests can drive
/// it with their own clock.
#[derive(Debug, Default)]
struct State {
    session: Option<Session>,
}

impl State {
    /// Starts `session`, replacing the running one, which is returned.
    fn start(&mut self, session: Session) -> Option<Session> {
        self.session.replace(session)
    }

    fn stop(&mut self) -> Option<Session> {
        self.session.take()
    }

    /// Ends the running session if its time is up, and returns it.
    fn expire(&mut self, now: DateTime<Local>) -> Option<Session> {
        match self.session {
            Some(session) if session.is_over(now) => self.session.take(),
            _ => None,
        }
    }
}

/// What a typed query asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    /// For this many minutes.
    For(u32),
    /// Until this time of day, today or else tomorrow.
    Until(NaiveTime),
}

impl Request {
    fn ends_at(self, now: DateTime<Local>) -> DateTime<Local> {
        match self {
            Self::For(minutes) => now + TimeDelta::minutes(minutes as i64),
            Self::Until(time) => next_occurrence(time, now),
        }
    }
}

/// The first `time` of day after `now`.
fn next_occurrence(time: NaiveTime, now: DateTime<Local>) -> DateTime<Local> {
    let at = |date: chrono::NaiveDate| date.and_time(time).and_local_timezone(Local).earliest();
    let today = now.date_naive();
    match at(today) {
        Some(at) if at > now => at,
        _ => today
            .succ_opt()
            .and_then(at)
            // A time skipped by a clock change: an hour later is near enough.
            .unwrap_or_else(|| now + TimeDelta::hours(1)),
    }
}

/// Reads a duration (`45`, `45m`, `2h`, `1h30m`, `90 min`) or a time of day
/// (`17:30`, `5pm`, `5:30 pm`).
fn parse_request(text: &str) -> Option<Request> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    parse_time(&text)
        .map(Request::Until)
        .or_else(|| parse_minutes(&text).map(Request::For))
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let (clock, afternoon) = match text.strip_suffix("pm").or(text.strip_suffix('p')) {
        Some(clock) => (clock, Some(true)),
        None => match text.strip_suffix("am").or(text.strip_suffix('a')) {
            Some(clock) => (clock, Some(false)),
            None => (text.as_str(), None),
        },
    };
    let (hours, minutes) = match clock.split_once([':', '.']) {
        Some((hours, minutes)) if minutes.len() == 2 => (hours, minutes.parse().ok()?),
        Some(_) => return None,
        // A bare number is a time only with am or pm; otherwise minutes.
        None if afternoon.is_some() => (clock, 0),
        None => return None,
    };
    let hours: u32 = hours.parse().ok()?;
    let hours = match afternoon {
        Some(_) if !(1..=12).contains(&hours) => return None,
        Some(true) => hours % 12 + 12,
        Some(false) => hours % 12,
        None => hours,
    };
    NaiveTime::from_hms_opt(hours, minutes, 0)
}

fn parse_minutes(text: &str) -> Option<u32> {
    let pattern =
        regex::Regex::new(r"^(?:(\d+)\s*h(?:ours?|rs?)?)?\s*(?:(\d+)\s*m(?:in(?:utes?|s)?)?)?$")
            .ok()?;
    let minutes = match text.parse::<u32>() {
        Ok(minutes) => minutes,
        Err(_) => {
            let captures = pattern.captures(text)?;
            let part = |index| {
                captures
                    .get(index)
                    .map_or(Some(0), |part| part.as_str().parse::<u32>().ok())
            };
            if captures.get(1).is_none() && captures.get(2).is_none() {
                return None;
            }
            part(1)?.checked_mul(60)?.checked_add(part(2)?)?
        }
    };
    (1..=LONGEST_MINUTES).contains(&minutes).then_some(minutes)
}

/// A length such as `15 min`, `1 h` or `1 h 30 min`.
fn format_minutes(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// How long is left, rounded up to the minute, so `1 min left` lasts until
/// the end.
fn format_remaining(remaining: TimeDelta) -> String {
    match remaining.num_seconds() {
        ..=0 => "Ending".into(),
        seconds => format!("{} left", format_minutes((seconds + 59) / 60)),
    }
}

/// When `ends_at` is, as briefly as `now` allows: `15:30`, `tomorrow 09:00`
/// or `Fri 09:00`.
fn format_until(ends_at: DateTime<Local>, now: DateTime<Local>) -> String {
    let time = ends_at.format("%H:%M");
    match (ends_at.date_naive() - now.date_naive()).num_days() {
        0 => time.to_string(),
        1 => format!("tomorrow {time}"),
        2..7 => format!("{} {time}", ends_at.format("%a")),
        _ => format!("{} {time}", ends_at.format("%b %-d")),
    }
}

/// The root search subtitle and the page's headline: `Awake` or
/// `Awake until 15:30`.
fn describe(session: &Session, now: DateTime<Local>) -> String {
    match session.ends_at {
        Some(ends_at) => format!("Awake until {}", format_until(ends_at, now)),
        None => "Awake".into(),
    }
}

/// The operating system's promise not to sleep, given back when dropped.
struct Inhibitor {
    #[cfg(target_os = "windows")]
    _release: std::sync::mpsc::Sender<()>,
    #[cfg(not(target_os = "windows"))]
    child: std::process::Child,
}

impl Inhibitor {
    #[cfg(target_os = "windows")]
    fn acquire(display: bool) -> Result<Self> {
        use std::sync::mpsc;
        use windows::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE,
            SetThreadExecutionState,
        };

        let (ready, is_ready) = mpsc::channel();
        let (release, is_released) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("keep-awake".into())
            .spawn(move || {
                let display = match display {
                    true => ES_DISPLAY_REQUIRED,
                    false => EXECUTION_STATE(0),
                };
                let previous = unsafe {
                    SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | display)
                };
                let granted = previous.0 != 0;
                ready.send(granted).ok();
                if granted {
                    // Returns when the sender is dropped.
                    is_released.recv().ok();
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                }
            })?;
        match is_ready.recv() {
            Ok(true) => Ok(Self { _release: release }),
            _ => anyhow::bail!("Windows refused to keep the computer awake"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn acquire(display: bool) -> Result<Self> {
        use std::process::{Command, Stdio};

        let mut command = inhibit_command(display, std::process::id());
        let program = command.remove(0);
        let mut child = Command::new(&program)
            .args(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| anyhow::anyhow!("cannot start `{program}`: {error}"))?;
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!("`{program}` ended at once ({status})");
        }
        Ok(Self { child })
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for Inhibitor {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// The program and arguments that keep the computer awake while they run,
/// and end by themselves with the launcher (process `launcher`).
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn inhibit_command(display: bool, launcher: u32) -> Vec<String> {
    let words: Vec<String> = match cfg!(target_os = "macos") {
        true => [
            "caffeinate",
            "-i",
            match display {
                true => "-d",
                false => "",
            },
            "-w",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([launcher.to_string()])
        .collect(),
        // `tail --pid` ends with the launcher, which `sleep infinity` would
        // outlive.
        false => [
            "systemd-inhibit",
            match display {
                true => "--what=idle:sleep",
                false => "--what=sleep",
            },
            "--who=Launcher",
            "--why=Keep Awake",
            "--mode=block",
            "tail",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([format!("--pid={launcher}"), "-f".into(), "/dev/null".into()])
        .collect(),
    };
    words.into_iter().filter(|word| !word.is_empty()).collect()
}

/// Whether this platform can keep the computer awake.
pub fn is_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::sources::process::is_installed("systemd-inhibit")
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(any(target_os = "windows", target_os = "macos"))
    }
}

/// The running session, the request that holds it, and its end timer.
pub struct KeepAwake {
    state: State,
    inhibitor: Option<Inhibitor>,
    _timer: Option<Task<()>>,
}

struct GlobalKeepAwake(Entity<KeepAwake>);

impl Global for GlobalKeepAwake {}

fn keep_awake(cx: &mut App) -> Entity<KeepAwake> {
    if let Some(global) = cx.try_global::<GlobalKeepAwake>() {
        return global.0.clone();
    }
    let entity = cx.new(|_| KeepAwake {
        state: State::default(),
        inhibitor: None,
        _timer: None,
    });
    cx.set_global(GlobalKeepAwake(entity.clone()));
    entity
}

/// Shows the state on the root search's "Keep Awake" row; `None` restores
/// its own subtitle.
fn set_subtitle(subtitle: Option<String>, cx: &mut App) {
    if let Some((_, host)) = crate::shell::launcher::host_and_catalog(cx) {
        host.set_command_subtitle(
            CommandId::new("system", COMMAND),
            subtitle.map(SharedString::from),
            cx,
        );
    }
}

impl KeepAwake {
    fn session(&self) -> Option<Session> {
        self.state.session
    }

    /// Starts `session`, replacing the running one without a gap, and says
    /// so in a HUD.
    fn begin(&mut self, session: Session, cx: &mut Context<Self>) {
        let inhibitor = match Inhibitor::acquire(session.display) {
            Ok(inhibitor) => inhibitor,
            Err(error) => {
                tracing::warn!("cannot keep the computer awake: {error:#}");
                perform(
                    Effect::ShowToast(
                        Toast::new(ToastStyle::Failure, "Couldn’t keep the computer awake")
                            .with_message(error.to_string()),
                    ),
                    cx,
                );
                return;
            }
        };
        self.state.start(session);
        // The previous request is withdrawn only now, after the new one holds.
        self.inhibitor = Some(inhibitor);
        self._timer = session.ends_at.map(|_| self.watch(cx));
        let now = Local::now();
        let headline = describe(&session, now);
        set_subtitle(Some(headline.clone()), cx);
        let hud = match session.display {
            true => format!("{headline} · Display on"),
            false => headline,
        };
        perform(Effect::ShowHud(hud.into()), cx);
        cx.notify();
    }

    /// Lets the computer sleep again; `hud` says why, if anything.
    fn end(&mut self, hud: Option<&str>, cx: &mut Context<Self>) {
        self.state.stop();
        self.finish(hud, cx);
    }

    fn finish(&mut self, hud: Option<&str>, cx: &mut Context<Self>) {
        self.inhibitor = None;
        self._timer = None;
        set_subtitle(None, cx);
        if let Some(hud) = hud {
            crate::shell::platform::show_hud(hud.to_owned().into(), cx);
        }
        cx.notify();
    }

    /// Ends the session when its time is up.
    fn watch(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let wait = this
                    .read_with(cx, |keep_awake, _| {
                        let remaining = keep_awake.session()?.remaining(Local::now())?;
                        Some(remaining.to_std().unwrap_or_default().min(CHECK))
                    })
                    .ok()
                    .flatten();
                let Some(wait) = wait else {
                    return;
                };
                cx.background_executor().timer(wait).await;
                let ended = this
                    .update(cx, |keep_awake, cx| {
                        let ended = keep_awake.state.expire(Local::now()).is_some();
                        if ended {
                            keep_awake.finish(Some("Your computer can sleep again"), cx);
                        }
                        ended
                    })
                    .unwrap_or(true);
                if ended {
                    return;
                }
            }
        })
    }
}

/// The root search commands this module offers on this platform.
pub fn commands() -> Vec<Item> {
    if !is_supported() {
        return Vec::new();
    }
    vec![
        crate::sources::system::command_item("system/keep-awake", "Keep Awake", "coffee")
            .with_keyword("caffeinate")
            .with_keyword("coffee")
            .with_keyword("amphetamine")
            .with_keyword("prevent sleep")
            .with_keyword("insomnia")
            .with_action(Action::new(
                "Keep Awake",
                Effect::Push(PushHandler::new(keep_awake_page)),
            )),
        crate::sources::system::command_item("system/allow-sleep", "Allow Sleep", "moon")
            .with_keyword("caffeinate")
            .with_keyword("coffee")
            .with_keyword("decaffeinate")
            .with_keyword("stop keep awake")
            .with_action(Action::new(
                "Allow Sleep",
                Effect::Run(RunHandler::new(|(), _, cx| {
                    let keep_awake = keep_awake(cx);
                    match keep_awake.read(cx).session().is_some() {
                        true => keep_awake.update(cx, |keep_awake, cx| {
                            keep_awake.end(Some("Your computer can sleep again"), cx)
                        }),
                        false => perform(
                            Effect::ShowHud("Your computer can already sleep".into()),
                            cx,
                        ),
                    }
                })),
            )),
    ]
}

pub fn keep_awake_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let keep_awake = keep_awake(cx);
    Ok(pages::handle(cx.new(|cx| {
        let subscription = cx.observe(&keep_awake, |_, _, cx| cx.notify());
        // Keeps the time left current while the page is open.
        let redraw = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REDRAW).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        KeepAwakePage {
            keep_awake,
            query: String::new(),
            _subscription: subscription,
            _redraw: redraw,
        }
    })))
}

struct KeepAwakePage {
    keep_awake: Entity<KeepAwake>,
    query: String,
    _subscription: gpui_kit::Subscription,
    _redraw: Task<()>,
}

impl KeepAwakePage {
    /// The two ways to start `ends_at`: with the display left to sleep, and
    /// with it kept on.
    fn start_actions(&self, ends_at: Option<DateTime<Local>>) -> ActionPanel {
        let start = |title: &str, icon: &str, display: bool| {
            let keep_awake = self.keep_awake.clone();
            Action::new(
                title.to_owned(),
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    keep_awake.update(cx, |keep_awake, cx| {
                        keep_awake.begin(Session { ends_at, display }, cx)
                    });
                })),
            )
            .with_image(Image::Icon(icon.into()))
        };
        ActionPanel::new()
            .with_action(start("Keep Awake", "coffee", false))
            .with_action(start("Keep Awake and Display On", "monitor", true))
    }

    fn current_item(&self, session: &Session, now: DateTime<Local>) -> Item {
        let keep_awake = self.keep_awake.clone();
        let item = Item::new(ItemId::new("keep-awake/current"), describe(session, now))
            .with_icon("coffee")
            .with_subtitle(match session.display {
                true => "Display stays on",
                false => "Display can sleep",
            })
            .with_action(
                Action::new(
                    "Allow Sleep",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        keep_awake.update(cx, |keep_awake, cx| {
                            keep_awake.end(Some("Your computer can sleep again"), cx)
                        });
                    })),
                )
                .with_image(Image::Icon("moon".into())),
            );
        match session.remaining(now) {
            Some(remaining) => {
                item.with_accessory(Accessory::tag(format_remaining(remaining), Tone::Accent))
            }
            None => item.with_accessory(Accessory::tag("Until stopped", Tone::Accent)),
        }
    }

    fn length_item(&self, minutes: Option<u32>, title: &str, now: DateTime<Local>) -> Item {
        let ends_at = minutes.map(|minutes| Request::For(minutes).ends_at(now));
        let item = Item::new(
            ItemId::new(format!("keep-awake/{}", minutes.unwrap_or(0))),
            title.to_owned(),
        )
        .with_icon(match minutes {
            None => "infinity",
            Some(_) => "timer",
        })
        .with_actions(self.start_actions(ends_at));
        match ends_at {
            Some(ends_at) => item.with_accessory(Accessory::text(format!(
                "Until {}",
                format_until(ends_at, now)
            ))),
            None => item.with_keyword("indefinitely"),
        }
    }

    /// The row for a typed duration or time.
    fn request_item(&self, request: Request, now: DateTime<Local>) -> Item {
        let ends_at = request.ends_at(now);
        let (title, accessory) = match request {
            Request::For(minutes) => (
                format!("For {}", format_minutes(minutes as i64)),
                format!("Until {}", format_until(ends_at, now)),
            ),
            Request::Until(_) => (
                format!("Until {}", format_until(ends_at, now)),
                format_minutes((ends_at - now).num_minutes().max(1)),
            ),
        };
        Item::new(ItemId::new("keep-awake/typed"), title)
            .with_icon("clock")
            .with_accessory(Accessory::text(accessory))
            .with_actions(self.start_actions(Some(ends_at)))
    }
}

impl Page for KeepAwakePage {
    fn title(&self) -> SharedString {
        "Keep Awake".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let now = Local::now();
        let session = self.keep_awake.read(cx).session();
        let list = ListModel::new().with_placeholder("Search, or type a duration or time");
        let list = match &session {
            Some(session) => list.with_section(
                Section::new()
                    .with_title("Now")
                    .with_item(self.current_item(session, now)),
            ),
            None => list,
        };
        let title = match session {
            Some(_) => "Replace With",
            None => "Keep Awake",
        };
        if let Some(request) = parse_request(&self.query) {
            return list
                .with_filtering(false)
                .with_section(
                    Section::new()
                        .with_title(title)
                        .with_item(self.request_item(request, now)),
                )
                .into();
        }
        let lengths = LENGTHS
            .iter()
            .map(|(minutes, title)| self.length_item(*minutes, title, now));
        list.with_section(
            Section::new()
                .with_title(title)
                .with_items(lengths)
                .with_item(
                    Item::new(ItemId::new("keep-awake/until"), "Until a Time…")
                        .with_icon("clock")
                        .with_action(Action::new(
                            "Choose a Time",
                            Effect::Push(PushHandler::new(until_time_page)),
                        )),
                ),
        )
        .into()
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.query = query.to_owned();
        cx.notify();
    }
}

mod field {
    pub const TIME: &str = "time";
    pub const DISPLAY: &str = "display";
}

fn until_time_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let keep_awake = keep_awake(cx);
    Ok(pages::handle(cx.new(|_| UntilTimePage { keep_awake })))
}

/// A form for an end time the list does not offer.
struct UntilTimePage {
    keep_awake: Entity<KeepAwake>,
}

impl Page for UntilTimePage {
    fn title(&self) -> SharedString {
        "Keep Awake Until".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let keep_awake = self.keep_awake.clone();
        let submit = FormHandler::new(move |values, _, cx| {
            let text = match values.get(field::TIME) {
                Some(FormValue::Text(text)) => text.to_string(),
                _ => String::new(),
            };
            let display = matches!(values.get(field::DISPLAY), Some(FormValue::Bool(true)));
            let Some(time) = parse_time(&text.trim().to_lowercase()) else {
                perform(
                    Effect::ShowToast(
                        Toast::new(ToastStyle::Failure, "Couldn’t read the time")
                            .with_message("Type a time such as 17:30 or 5:30 pm."),
                    ),
                    cx,
                );
                return;
            };
            let ends_at = Request::Until(time).ends_at(Local::now());
            keep_awake.update(cx, |keep_awake, cx| {
                keep_awake.begin(
                    Session {
                        ends_at: Some(ends_at),
                        display,
                    },
                    cx,
                )
            });
        });
        FormModel::new()
            .with_field(Field::new(
                field::TIME,
                "Until",
                Control::Text {
                    placeholder: Some("17:30".into()),
                    value: SharedString::default(),
                },
            ))
            .with_field(Field::new(
                field::DISPLAY,
                "",
                Control::Checkbox {
                    label: "Keep display on".into(),
                    value: false,
                },
            ))
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Keep Awake", Effect::SubmitForm(submit))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, 10, hour, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn test_formats_lengths_and_times() {
        assert_eq!(format_minutes(15), "15 min");
        assert_eq!(format_minutes(60), "1 h");
        assert_eq!(format_minutes(90), "1 h 30 min");
        assert_eq!(format_remaining(TimeDelta::seconds(61)), "2 min left");
        assert_eq!(format_remaining(TimeDelta::minutes(60)), "1 h left");
        assert_eq!(format_remaining(TimeDelta::zero()), "Ending");

        let now = at(14, 0);
        assert_eq!(format_until(at(15, 30), now), "15:30");
        assert_eq!(
            format_until(at(9, 0) + TimeDelta::days(1), now),
            "tomorrow 09:00"
        );
        let session = Session {
            ends_at: Some(at(15, 30)),
            display: false,
        };
        assert_eq!(describe(&session, now), "Awake until 15:30");
        let session = Session {
            ends_at: None,
            display: true,
        };
        assert_eq!(describe(&session, now), "Awake");
    }

    #[test]
    fn test_parses_durations_and_times() {
        assert_eq!(parse_request("45"), Some(Request::For(45)));
        assert_eq!(parse_request("45m"), Some(Request::For(45)));
        assert_eq!(parse_request("90 min"), Some(Request::For(90)));
        assert_eq!(parse_request("2h"), Some(Request::For(120)));
        assert_eq!(parse_request("1h30m"), Some(Request::For(90)));
        assert_eq!(parse_request("1 hour 15 minutes"), Some(Request::For(75)));
        let time = |hour, minute| {
            Some(Request::Until(
                NaiveTime::from_hms_opt(hour, minute, 0).unwrap(),
            ))
        };
        assert_eq!(parse_request("17:30"), time(17, 30));
        assert_eq!(parse_request("5pm"), time(17, 0));
        assert_eq!(parse_request("5:30 PM"), time(17, 30));
        assert_eq!(parse_request("12am"), time(0, 0));
        assert_eq!(parse_request("12pm"), time(12, 0));
        for nothing in ["", "0", "hour", "15 days", "25:00", "13pm", "5:3", "99999"] {
            assert_eq!(parse_request(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn test_end_times() {
        let now = at(14, 0);
        assert_eq!(Request::For(90).ends_at(now), at(15, 30));
        let time = |hour, minute| NaiveTime::from_hms_opt(hour, minute, 0).unwrap();
        // A time still ahead is today; one already past is tomorrow.
        assert_eq!(Request::Until(time(17, 30)).ends_at(now), at(17, 30));
        assert_eq!(
            Request::Until(time(9, 0)).ends_at(now),
            at(9, 0) + TimeDelta::days(1)
        );
        assert_eq!(
            Request::Until(time(14, 0)).ends_at(now),
            at(14, 0) + TimeDelta::days(1)
        );
    }

    #[test]
    fn test_state_starts_replaces_expires_and_stops() {
        let mut state = State::default();
        let hour = Session {
            ends_at: Some(at(15, 0)),
            display: false,
        };
        assert_eq!(state.start(hour), None);
        assert_eq!(state.expire(at(14, 59)), None, "not yet over");
        assert_eq!(
            state.session.unwrap().remaining(at(14, 30)),
            Some(TimeDelta::minutes(30))
        );

        let forever = Session {
            ends_at: None,
            display: true,
        };
        assert_eq!(state.start(forever), Some(hour), "replaces the running one");
        assert_eq!(state.expire(at(23, 59)), None, "never ends by itself");
        assert_eq!(state.stop(), Some(forever));
        assert_eq!(state.stop(), None);

        state.start(hour);
        assert_eq!(state.expire(at(15, 0)), Some(hour));
        assert_eq!(state.session, None);
        assert_eq!(state.expire(at(16, 0)), None, "expires once");
    }

    #[test]
    fn test_inhibit_command_ends_with_the_launcher() {
        let words = inhibit_command(true, 42);
        if cfg!(target_os = "macos") {
            assert_eq!(words, ["caffeinate", "-i", "-d", "-w", "42"]);
            assert_eq!(inhibit_command(false, 42), ["caffeinate", "-i", "-w", "42"]);
        } else {
            assert_eq!(words[0], "systemd-inhibit");
            assert!(words.contains(&"--what=idle:sleep".to_owned()));
            assert!(words.ends_with(&[
                "tail".into(),
                "--pid=42".into(),
                "-f".into(),
                "/dev/null".into()
            ]));
            assert!(inhibit_command(false, 42).contains(&"--what=sleep".to_owned()));
        }
    }
}
