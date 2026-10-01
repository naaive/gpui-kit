//! Focus: a session with a goal and a length, during which distracting
//! applications and websites are kept out of the way, as Raycast Focus does.
//!
//! While a session runs, the window in front is checked every second. A
//! blocked application is minimized; a browser showing a blocked site is
//! too, its address read through the accessibility API. Either way a short
//! message says why. The session is saved, so it survives a restart, and
//! the root search shows it with the time left.
//!
//! Windows only; elsewhere the commands are not offered.

#[cfg(target_os = "windows")]
pub(crate) mod browser;

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task, Window};
use serde::{Deserialize, Serialize};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Choice, Control,
        Effect, Field, FormHandler, FormModel, FormValue, FormValues, Image, Item, ItemId,
        PageModel, PushHandler, RunHandler, Tone,
    },
    pages::{self, Page, PageHandle},
    search::now,
    shell::{launcher::perform, platform::show_hud},
};

/// How often the window in front is checked.
const CHECK: Duration = Duration::from_secs(1);
/// A blocked application is announced at most this often.
const ANNOUNCE_AGAIN: u64 = 10;

/// Distractions, grouped as the start form offers them.
const CATEGORIES: [(&str, &str, &[&str], &[&str]); 5] = [
    (
        "social",
        "Social Media",
        &[],
        &[
            "x.com",
            "twitter.com",
            "facebook.com",
            "instagram.com",
            "reddit.com",
            "weibo.com",
            "tiktok.com",
            "douyin.com",
            "xiaohongshu.com",
            "zhihu.com",
        ],
    ),
    (
        "video",
        "Video",
        &[],
        &[
            "youtube.com",
            "bilibili.com",
            "netflix.com",
            "twitch.tv",
            "iqiyi.com",
            "youku.com",
        ],
    ),
    (
        "news",
        "News",
        &[],
        &[
            "news.ycombinator.com",
            "news.google.com",
            "cnn.com",
            "bbc.com",
            "nytimes.com",
            "toutiao.com",
        ],
    ),
    (
        "gaming",
        "Gaming",
        &[
            "steam",
            "epicgameslauncher",
            "battle.net",
            "riotclientservices",
        ],
        &["store.steampowered.com", "epicgames.com"],
    ),
    (
        "chat",
        "Chat",
        &["discord", "telegram", "wechat", "weixin", "qq", "whatsapp"],
        &["discord.com", "web.telegram.org", "web.whatsapp.com"],
    ),
];

const DURATIONS: [(u64, &str); 7] = [
    (10, "10 minutes"),
    (25, "25 minutes"),
    (50, "50 minutes"),
    (60, "1 hour"),
    (90, "1 hour 30 minutes"),
    (120, "2 hours"),
    (0, "Until I stop it"),
];

/// A running focus session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Session {
    pub goal: String,
    /// Unix seconds; `None` runs until it is completed.
    ends_at: Option<u64>,
    /// When it was paused, if it is.
    paused_at: Option<u64>,
    /// Executable names, lowercase, without `.exe`.
    apps: Vec<String>,
    /// Domains; a site's subdomains are blocked with it.
    sites: Vec<String>,
}

impl Session {
    /// Seconds left, from `now`; `None` for a session without an end.
    pub fn remaining(&self, now: u64) -> Option<u64> {
        let ends_at = self.ends_at?;
        let now = self.paused_at.unwrap_or(now);
        Some(ends_at.saturating_sub(now))
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Whether the application `name` showing `address` is blocked.
    fn blocks(&self, name: &str, address: Option<&str>) -> bool {
        if self.apps.iter().any(|app| app.eq_ignore_ascii_case(name)) {
            return true;
        }
        let Some(address) = address else {
            return false;
        };
        let address = address.trim().to_lowercase();
        let address = address
            .split_once("://")
            .map_or(address.as_str(), |(_, rest)| rest);
        let host = address.split(['/', '?', '#', ':']).next().unwrap_or("");
        let host = host.strip_prefix("www.").unwrap_or(host);
        self.sites.iter().any(|site| {
            let site = site.trim().trim_start_matches("www.").to_lowercase();
            // A path, such as reddit.com/r/all, blocks only that part.
            match site.split_once('/') {
                Some((domain, _)) => {
                    (host == domain || host.ends_with(&format!(".{domain}")))
                        && address
                            .strip_prefix("www.")
                            .unwrap_or(address)
                            .starts_with(&site)
                }
                None => host == site || host.ends_with(&format!(".{site}")),
            }
        })
    }
}

/// `1:05:09`, `24:59`.
pub fn clock(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

/// What is saved between runs: the session, and the choices last made in
/// the start form.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Saved {
    session: Option<Session>,
    categories: Vec<String>,
    apps: String,
    sites: String,
    minutes: u64,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("focus.json"))
}

impl Saved {
    fn load() -> Self {
        path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| Self {
                categories: vec!["social".into(), "video".into()],
                minutes: 25,
                ..Self::default()
            })
    }

    fn save(&self) {
        let Some(path) = path() else {
            return;
        };
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).ok();
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            std::fs::write(path, text).ok();
        }
    }
}

/// The session and the task that enforces it.
pub struct Focus {
    saved: Saved,
    watch: Option<Task<()>>,
    /// The application announced last, and when, so a HUD is not shown
    /// every second.
    announced: Option<(String, u64)>,
}

struct GlobalFocus(Entity<Focus>);

impl Global for GlobalFocus {}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

/// The focus state, created on first use.
pub fn focus(cx: &mut App) -> Entity<Focus> {
    if let Some(focus) = cx.try_global::<GlobalFocus>() {
        return focus.0.clone();
    }
    let focus = cx.new(|_| Focus {
        saved: Saved::load(),
        watch: None,
        announced: None,
    });
    cx.set_global(GlobalFocus(focus.clone()));
    focus
}

/// Reads the saved session and choices again, after an import.
pub fn reload(cx: &mut App) {
    let Some(focus) = cx.try_global::<GlobalFocus>().map(|focus| focus.0.clone()) else {
        return;
    };
    focus.update(cx, |focus, cx| {
        // An export carries no session; the one running here goes on.
        let session = focus.saved.session.take();
        focus.saved = Saved::load();
        focus.saved.session = session;
        focus.saved.save();
        cx.notify();
    });
}

/// Resumes a session saved by an earlier run.
pub fn start(cx: &mut App) {
    if !is_supported() {
        return;
    }
    let focus = focus(cx);
    focus.update(cx, |focus, cx| focus.watch(cx));
}

impl Focus {
    pub fn session(&self) -> Option<&Session> {
        self.saved.session.as_ref()
    }

    fn begin(&mut self, session: Session, cx: &mut Context<Self>) {
        self.saved.session = Some(session);
        self.saved.save();
        self.watch(cx);
        cx.notify();
    }

    /// Ends the session, as completed or stopped.
    fn end(&mut self, message: &str, cx: &mut Context<Self>) {
        if self.saved.session.take().is_some() {
            self.saved.save();
            self.watch = None;
            show_hud(message.to_owned().into(), cx);
            cx.notify();
        }
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.saved.session.as_mut() else {
            return;
        };
        let now = now();
        match session.paused_at.take() {
            // The pause does not count against the session.
            Some(paused_at) => {
                session.ends_at = session
                    .ends_at
                    .map(|ends_at| ends_at + now.saturating_sub(paused_at))
            }
            None => session.paused_at = Some(now),
        }
        self.saved.save();
        cx.notify();
    }

    fn extend(&mut self, minutes: u64, cx: &mut Context<Self>) {
        if let Some(session) = self.saved.session.as_mut()
            && let Some(ends_at) = session.ends_at.as_mut()
        {
            // While paused the clock stands at `paused_at`, and resuming
            // adds the pause, so only the minutes are added here.
            let reference = session.paused_at.unwrap_or_else(now);
            *ends_at = (*ends_at).max(reference) + minutes * 60;
            self.saved.save();
            cx.notify();
        }
    }

    /// Checks the window in front every second while a session runs.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.saved.session.is_none() || self.watch.is_some() {
            return;
        }
        self.watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK).await;
                let Ok(session) = this.update(cx, |focus, cx| {
                    let session = focus.saved.session.clone();
                    if let Some(session) = &session
                        && session.remaining(now()) == Some(0)
                    {
                        focus.end("Focus session complete", cx);
                        return None;
                    }
                    // The root search shows the time left.
                    cx.notify();
                    session
                }) else {
                    return;
                };
                let Some(session) = session else {
                    return;
                };
                if session.is_paused() {
                    continue;
                }
                let blocked = cx
                    .background_spawn(async move { blocked_front_window(&session) })
                    .await;
                if let Some(name) = blocked {
                    this.update(cx, |focus, cx| focus.announce(name, cx)).ok();
                }
            }
        }));
    }

    fn announce(&mut self, name: String, cx: &mut Context<Self>) {
        let now = now();
        let recent = self.announced.as_ref().is_some_and(|(announced, at)| {
            *announced == name && now.saturating_sub(*at) < ANNOUNCE_AGAIN
        });
        if !recent {
            let goal = self
                .saved
                .session
                .as_ref()
                .map(|session| session.goal.clone())
                .unwrap_or_default();
            show_hud(
                match goal.is_empty() {
                    true => format!("{name} is blocked while you focus"),
                    false => format!("{name} is blocked · {goal}"),
                }
                .into(),
                cx,
            );
            self.announced = Some((name, now));
        }
    }
}

/// Minimizes the window in front when the session blocks it; returns what
/// was blocked.
fn blocked_front_window(session: &Session) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, SW_MINIMIZE, ShowWindow,
        };
        let window = unsafe { GetForegroundWindow() };
        let exe = crate::switch_windows::window_executable(window.0 as isize)?;
        let name = exe.file_stem()?.to_string_lossy().to_lowercase();
        let address = match browser::is_browser(&name) {
            true => browser::address(window.0 as isize),
            false => None,
        };
        if !session.blocks(&name, address.as_deref()) {
            return None;
        }
        unsafe {
            let _ = ShowWindow(window, SW_MINIMIZE);
        }
        Some(match address {
            Some(address) if !session.apps.contains(&name) => address
                .split("://")
                .last()
                .unwrap_or(&address)
                .split('/')
                .next()
                .unwrap_or(&address)
                .to_owned(),
            _ => exe
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or(name),
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = session;
        None
    }
}

/// The root search's row for the running session, with its controls.
pub fn session_item(cx: &mut App) -> Option<Item> {
    let focus = cx.try_global::<GlobalFocus>()?.0.clone();
    let session = focus.read(cx).session()?.clone();
    let remaining = session.remaining(now());
    let control = |title: &str, icon: &str, run: fn(&mut Focus, &mut Context<Focus>)| {
        let focus = focus.clone();
        Action::new(
            title.to_owned(),
            Effect::Run(RunHandler::new(move |(), _, cx| {
                focus.update(cx, |focus, cx| run(focus, cx));
            })),
        )
        .with_image(Image::Icon(icon.into()))
    };
    let mut actions = ActionPanel::new().with_action(control(
        "Complete Focus Session",
        "circle-check",
        |focus, cx| focus.end("Focus session complete", cx),
    ));
    actions = actions.with_action(match session.is_paused() {
        true => control("Resume Focus Session", "play", Focus::toggle_pause),
        false => control("Pause Focus Session", "pause", Focus::toggle_pause),
    });
    if remaining.is_some() {
        actions = actions.with_action(
            control("Add 5 Minutes", "plus", |focus, cx| focus.extend(5, cx))
                .with_shortcut("secondary-plus"),
        );
    }
    let blocked = format!(
        "Blocking {} apps and {} sites",
        session.apps.len(),
        session.sites.len()
    );
    let item = Item::new(
        ItemId::new("focus/session"),
        match session.goal.is_empty() {
            true => "Focus".to_owned(),
            false => session.goal.clone(),
        },
    )
    .with_icon("target")
    .with_subtitle(blocked)
    .with_actions(actions);
    Some(match (remaining, session.is_paused()) {
        (_, true) => item.with_accessory(Accessory::tag("Paused", Tone::Warning)),
        (Some(remaining), false) => item.with_accessory(Accessory::tag(
            format!("{} left", clock(remaining)),
            Tone::Accent,
        )),
        (None, false) => item.with_accessory(Accessory::tag("Focusing", Tone::Accent)),
    })
}

/// The root search commands.
pub fn commands() -> Vec<Item> {
    if !is_supported() {
        return Vec::new();
    }
    vec![
        Item::new(ItemId::new("system/start-focus"), "Start Focus Session")
            .with_icon("target")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("focus")
            .with_keyword("block")
            .with_keyword("distraction")
            .with_keyword("pomodoro")
            .with_action(Action::new(
                "Start Focus Session",
                Effect::Push(PushHandler::new(start_focus_page)),
            )),
        Item::new(
            ItemId::new("system/complete-focus"),
            "Complete Focus Session",
        )
        .with_icon("circle-check")
        .with_accessory(Accessory::text("Command"))
        .with_keyword("focus")
        .with_keyword("stop")
        .with_action(Action::new(
            "Complete Focus Session",
            Effect::Run(RunHandler::new(|(), _, cx| {
                let focus = focus(cx);
                match focus.read(cx).session().is_some() {
                    true => focus.update(cx, |focus, cx| focus.end("Focus session complete", cx)),
                    false => show_hud("No focus session is running".into(), cx),
                }
            })),
        )),
    ]
}

mod field {
    pub const GOAL: &str = "goal";
    pub const DURATION: &str = "duration";
    pub const APPS: &str = "apps";
    pub const SITES: &str = "sites";
}

fn category_field(id: &str) -> String {
    format!("category:{id}")
}

pub fn start_focus_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let focus = focus(cx);
    Ok(pages::handle(cx.new(|_| StartFocusPage { focus })))
}

struct StartFocusPage {
    focus: Entity<Focus>,
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

impl StartFocusPage {
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let minutes: u64 = text(&values, field::DURATION).parse().unwrap_or(25);
        let chosen: Vec<&str> = CATEGORIES
            .iter()
            .filter(|(id, ..)| {
                matches!(
                    values.get(category_field(id).as_str()),
                    Some(FormValue::Bool(true))
                )
            })
            .map(|(id, ..)| *id)
            .collect();
        let custom_apps = text(&values, field::APPS);
        let custom_sites = text(&values, field::SITES);
        let mut apps = crate::shell::settings::parse_app_list(&custom_apps);
        let mut sites: Vec<String> = custom_sites
            .split([',', '\n', ' '])
            .map(|site| {
                let site = site.trim().to_lowercase();
                let site = site
                    .split_once("://")
                    .map_or(site.as_str(), |(_, rest)| rest);
                site.trim_end_matches('/').to_owned()
            })
            .filter(|site| !site.is_empty())
            .collect();
        for (id, _, category_apps, category_sites) in CATEGORIES {
            if chosen.contains(&id) {
                apps.extend(category_apps.iter().map(|app| (*app).to_owned()));
                sites.extend(category_sites.iter().map(|site| (*site).to_owned()));
            }
        }
        apps.sort();
        apps.dedup();
        sites.sort();
        sites.dedup();
        let goal = text(&values, field::GOAL);
        let session = Session {
            goal: goal.clone(),
            ends_at: (minutes > 0).then(|| now() + minutes * 60),
            paused_at: None,
            apps,
            sites,
        };
        self.focus.update(cx, |focus, cx| {
            focus.saved.categories = chosen.iter().map(|id| (*id).to_owned()).collect();
            focus.saved.apps = custom_apps;
            focus.saved.sites = custom_sites;
            focus.saved.minutes = minutes;
            focus.begin(session, cx);
        });
        let message = match (minutes, goal.is_empty()) {
            (0, _) => "Focus session started".to_owned(),
            (minutes, true) => format!("Focusing for {minutes} min"),
            (minutes, false) => format!("{goal} · {minutes} min"),
        };
        perform(Effect::ShowHud(message.into()), cx);
    }
}

impl Page for StartFocusPage {
    fn title(&self) -> SharedString {
        "Start Focus Session".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let saved = self.focus.read(cx).saved.clone();
        let running = saved.session.is_some();
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let form = FormModel::new()
            .with_field(Field::new(
                field::GOAL,
                "Goal",
                Control::Text {
                    placeholder: Some("Finish the report".into()),
                    value: SharedString::default(),
                },
            ))
            .with_field(Field::new(
                field::DURATION,
                "Duration",
                Control::Dropdown {
                    choices: DURATIONS
                        .iter()
                        .map(|(minutes, title)| Choice::new(minutes.to_string(), *title))
                        .collect(),
                    value: Some(saved.minutes.to_string().into()),
                },
            ));
        let form = CATEGORIES
            .iter()
            .enumerate()
            .fold(form, |form, (index, (id, title, apps, sites))| {
                let examples: Vec<&str> =
                    sites.iter().chain(apps.iter()).take(3).copied().collect();
                form.with_field(
                    Field::new(
                        category_field(id),
                        match index {
                            0 => "Block",
                            _ => "",
                        },
                        Control::Checkbox {
                            label: (*title).into(),
                            value: saved.categories.iter().any(|chosen| chosen == id),
                        },
                    )
                    .with_info(format!("{}…", examples.join(", "))),
                )
            })
            .with_field(
                Field::new(
                    field::APPS,
                    "Also block apps",
                    Control::Text {
                        placeholder: Some("spotify, wechat".into()),
                        value: saved.apps.clone().into(),
                    },
                )
                .with_info("Program names, separated by commas."),
            )
            .with_field(
                Field::new(
                    field::SITES,
                    "Also block websites",
                    Control::Text {
                        placeholder: Some("reddit.com, news.ycombinator.com".into()),
                        value: saved.sites.clone().into(),
                    },
                )
                .with_info("A site blocks its subdomains too; reddit.com/r/all only that part."),
            );
        let start = Action::new(
            match running {
                true => "Replace Running Session",
                false => "Start Focus Session",
            },
            Effect::SubmitForm(submit),
        );
        let mut actions = ActionPanel::new().with_action(start);
        if running {
            let focus = self.focus.clone();
            actions = actions.with_section(
                ActionSection::new().with_entry(ActionEntry::Action(
                    Action::new(
                        "Complete Running Session",
                        Effect::Run(RunHandler::new(move |(), _, cx| {
                            focus.update(cx, |focus, cx| focus.end("Focus session complete", cx));
                        })),
                    )
                    .with_style(ActionStyle::Destructive),
                )),
            );
        }
        form.with_actions(actions).into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(apps: &[&str], sites: &[&str]) -> Session {
        Session {
            goal: String::new(),
            ends_at: Some(1_000),
            paused_at: None,
            apps: apps.iter().map(|app| (*app).to_owned()).collect(),
            sites: sites.iter().map(|site| (*site).to_owned()).collect(),
        }
    }

    #[test]
    fn test_session_blocks_apps_sites_and_subdomains() {
        let session = session(&["steam"], &["youtube.com", "reddit.com/r/all"]);
        assert!(session.blocks("Steam", None));
        assert!(!session.blocks("code", None));
        assert!(session.blocks("chrome", Some("https://www.youtube.com/watch?v=1")));
        assert!(session.blocks("chrome", Some("m.youtube.com")));
        assert!(!session.blocks("chrome", Some("notyoutube.com")));
        assert!(session.blocks("msedge", Some("reddit.com/r/all/top")));
        assert!(!session.blocks("msedge", Some("reddit.com/r/rust")));
        assert!(!session.blocks("chrome", Some("github.com")));
    }

    #[test]
    fn test_remaining_time_and_pause() {
        let mut running = session(&[], &[]);
        assert_eq!(running.remaining(400), Some(600));
        assert_eq!(running.remaining(2_000), Some(0));
        running.paused_at = Some(500);
        assert_eq!(
            running.remaining(900),
            Some(500),
            "paused time does not pass"
        );
        assert_eq!(clock(3_909), "1:05:09");
        assert_eq!(clock(1_499), "24:59");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod by_hand {
    /// Prints the host each open browser window shows, to try the address
    /// reader by hand.
    #[test]
    #[ignore = "needs a browser window"]
    fn test_read_browser_addresses() {
        for window in crate::switch_windows::open_windows() {
            let Some(name) = window.exe.as_ref().and_then(|exe| exe.file_stem()) else {
                continue;
            };
            let name = name.to_string_lossy().to_lowercase();
            if super::browser::is_browser(&name) {
                let started = std::time::Instant::now();
                let address = super::browser::address(window.handle);
                let host = address
                    .as_deref()
                    .map(|address| address.split("://").last().unwrap_or(address))
                    .and_then(|rest| rest.split('/').next())
                    .map(str::to_owned);
                println!("{name}: {host:?} in {:?}", started.elapsed());
            }
        }
    }
}
