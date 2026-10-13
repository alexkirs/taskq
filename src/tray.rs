//! `taskq tray` (taskq.md § 4): the host's one menu-bar / notification-area icon over the `<root>/.taskq/status.json`
//! and `events` of every project in `<install>/projects`. A click opens `tray.html` in a wry webview; Rust pushes one
//! JSON state, the page sends actions back. No board access and no token: actions go through the same files and
//! commands as the CLI.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use tray_icon::{Icon, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{WebView, WebViewBuilder};

const HTML: &str = include_str!("tray.html");
const FONTS: [u64; 4] = [14, 16, 18, 20];
const PERIODS: [(&str, i64); 5] = [("5m", 300), ("1h", 3600), ("1d", 86400), ("1w", 604800), ("1mo", 2592000)];

/// False when another tray wrote the heartbeat `<install>/tray` in the last 10 s (it ticks every 2 s): of two trays
/// started at once, the first to see the other's pid exits.
fn alone(install: &Path) -> bool {
    let hb = install.join("tray");
    let other = fs::read_to_string(&hb).ok().and_then(|p| p.trim().parse::<u32>().ok()).is_some_and(|p| p != std::process::id());
    !(other && crate::file_age(&hb).is_some_and(|a| a < 10))
}

enum Ev {
    Tick,
    Ipc(String),
    Click(Rect),
    Shot,
    Shot2,
    /// Add project: the picked folder classified, or Null (cancel)
    Added(Value),
}

/// View, text size and period of `<install>/tray.json`; anything else falls back to the default. A `custom` period
/// is `[start, end)` in epoch seconds with the time zone `local` or `UTC` it was typed in.
#[derive(Clone, PartialEq)]
struct Settings {
    view: String,
    font: u64,
    period: String,
    start: i64,
    end: i64,
    tz: String,
}

impl Settings {
    fn load(install: &Path) -> Settings {
        let v: Value = fs::read_to_string(install.join("tray.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into() };
        s.set(&v);
        s
    }

    fn set(&mut self, v: &Value) {
        if let Some(x) = v["view"].as_str().filter(|x| ["icon", "full"].contains(x)) {
            self.view = x.into();
        }
        if let Some(x) = v["font"].as_u64().filter(|x| FONTS.contains(x)) {
            self.font = x;
        }
        if let Some(x) = v["period"].as_str().filter(|x| PERIODS.iter().any(|p| p.0 == *x)) {
            self.period = x.into();
        }
        // a custom range with start >= end is refused (menu.md M6)
        if let (Some("custom"), Some(a), Some(b)) = (v["period"].as_str(), v["start"].as_i64(), v["end"].as_i64())
            && a < b
        {
            (self.period, self.start, self.end) = ("custom".into(), a, b);
            self.tz = if v["tz"] == "UTC" { "UTC" } else { "local" }.into();
        }
    }

    /// `[start, end)` of the period at `now`.
    fn range(&self, now: i64) -> (i64, i64) {
        if self.period == "custom" {
            return (self.start, self.end);
        }
        (now - PERIODS.iter().find(|p| p.0 == self.period).map_or(86400, |p| p.1), now)
    }

    fn save(&self, install: &Path) {
        let v = json!({"view": self.view, "font": self.font, "period": self.period, "start": self.start, "end": self.end, "tz": self.tz});
        let _ = crate::platform::write_atomic(&install.join("tray.json"), &v.to_string());
    }
}

/// Draft and history of AI requests (§ 4): `<install>/tray-requests.json`, apart from the view settings.
fn requests_file(install: &Path) -> Value {
    let v = crate::operator::read(&install.join("tray-requests.json"));
    json!({"draft": v["draft"].as_str().unwrap_or(""), "history": v["history"].as_array().cloned().unwrap_or_default()})
}

/// The last 20 distinct sent texts, newest first; the same text moves to the top (menu.md M7).
fn remember(history: &[Value], text: &str) -> Vec<Value> {
    std::iter::once(json!(text)).chain(history.iter().filter(|h| h.as_str() != Some(text)).cloned()).take(crate::operator::KEEP).collect()
}

/// The project whose service takes a new request: the first registered one with a fresh `status.json`.
fn target(install: &Path) -> Option<String> {
    roots(install).into_iter().find(|r| crate::file_age(&crate::dot(Path::new(r)).join("status.json")).is_some_and(|a| a < 300))
}

/// The newest request over all projects with its answer.
fn newest(install: &Path) -> Value {
    let all = roots(install).into_iter().filter_map(|r| crate::operator::ids(Path::new(&r)).pop().map(|id| (id, r)));
    let Some((id, r)) = all.max_by_key(|(id, _)| (id.len(), id.clone())) else { return Value::Null };
    let root = Path::new(&r);
    let mut chain = Vec::new();
    let mut current = id.clone();
    for _ in 0..crate::operator::KEEP {
        let q = crate::operator::read(&crate::operator::dir(root).join(format!("{current}.json")));
        if q.is_null() { break; }
        chain.push(json!({"id": current, "text": q["text"], "answer": crate::operator::read(&crate::operator::answer_path(root, &current))}));
        let Some(after) = q["after"].as_str().filter(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit())) else { break };
        if chain.iter().any(|q| q["id"] == after) { break; }
        current = after.to_string();
    }
    chain.reverse();
    let mut request = chain.last().cloned().unwrap_or(Value::Null);
    request["root"] = json!(r);
    request["chain"] = json!(chain);
    request
}

fn ai(install: &Path, a: &str, m: &Value) {
    let path = install.join("tray-requests.json");
    let mut f = requests_file(install);
    match a {
        "ai-draft" => f["draft"] = json!(m["text"].as_str().unwrap_or("")),
        "ai-clear" => f["history"] = json!([]),
        "ai-send" => {
            let (Some(text), Some(r)) = (m["text"].as_str().map(str::trim).filter(|t| !t.is_empty()), target(install)) else { return };
            let onboard = m["onboard"].as_str();
            let root = Path::new(&r);
            let id = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string();
            let after = m["after"].as_str().filter(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
            let _ = fs::create_dir_all(crate::operator::dir(root));
            if crate::platform::write_atomic(&crate::operator::dir(root).join(format!("{id}.json")), &json!({"text": text, "after": after, "onboard": onboard}).to_string()).is_err() {
                return;
            }
            crate::poke(root);
            if onboard.is_some() {
                return;
            }
            f = json!({"draft": "", "history": remember(f["history"].as_array().map_or(&[][..], |v| v), text)});
        }
        // only a registered project and a request id: the page never names a path of its own
        "ai-confirm" => {
            let (r, id) = (m["root"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""));
            if roots(install).iter().any(|x| x == r) && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                let _ = fs::write(crate::operator::dir(Path::new(r)).join(format!("{id}.confirm")), m["rev"].as_u64().unwrap_or(0).to_string());
                crate::poke(Path::new(r));
            }
            return;
        }
        _ => return,
    }
    let _ = crate::platform::write_atomic(&path, &f.to_string());
}

fn roots(install: &Path) -> Vec<String> {
    fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect()
}

/// One project for the page: its `status.json`, saved focus, labels and `events` lines `[time, kind, attempt]`.
fn project(root: &str) -> Value {
    let r = Path::new(root);
    let d = crate::dot(r);
    let s: Value = fs::read_to_string(d.join("status.json")).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(Value::Null);
    let age = crate::file_age(&d.join("status.json"));
    let saved = fs::read_to_string(d.join("focus.paused")).is_ok_and(|f| !matches!(f.trim(), "" | "off"));
    let text = fs::read_to_string(d.join("events")).unwrap_or_default();
    let since = text.lines().next().and_then(|l| l.strip_prefix("since ")?.parse::<i64>().ok());
    let events: Vec<Value> = text.lines().skip(1).filter_map(|l| {
        let w: Vec<&str> = l.split(' ').collect();
        Some(json!([w.first()?.parse::<i64>().ok()?, w.get(2)?, w.get(3)?]))
    }).collect();
    // the local host's saved capacity, for `Edit limits` and its readback (§ 4 Capacity)
    let (cap_host, cap_saved) = match crate::load(r) {
        Ok((c, v)) => (json!(c.me), crate::service::capacity(&v, &c.me).map(|x| x.json()).unwrap_or_else(|e| json!({"error": e}))),
        Err(e) => (Value::Null, json!({"error": e})),
    };
    let tasks: Vec<Value> = s["tasks"].as_array().into_iter().flatten().map(|t| json!({"n": t["n"].as_u64().unwrap_or(0), "title": t["title"].as_str().unwrap_or(""), "state": t["state"].as_str().unwrap_or("")})).collect();
    json!({"root": root, "name": s["project"].as_str().map(String::from).unwrap_or_else(|| crate::project_name(r)),
        "issues": s["issues"].as_str().unwrap_or(""), "log": s["log"].is_string(), "alive": age.is_some_and(|a| a < 300), "age": age,
        "error": s["error"], "focus": s["focus"].as_array().cloned().unwrap_or_default(), "saved": saved, "labels": crate::status_labels(r),
        "request": s["request"], "since": since, "events": events, "tasks": tasks, "host": s["host"], "version": s["version"], "update": s["update"],
        "capacity": s["capacity"], "cap_host": cap_host, "cap_saved": cap_saved, "agents": s["agents"]})
}

fn now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// What the icon and the history show: in work, `ask`, ready over all projects (one task per board and issue), whether
/// any project admits, 12 equal bins of `[start, end)` (oldest first) counting `KINDS` and the latest `since` of the
/// projects that have history (None: none has).
#[derive(Clone)]
struct Summary {
    counts: [usize; 3],
    run: bool,
    bins: Vec<[u32; 4]>,
    since: Option<i64>,
}

/// Event kinds of the history, in stacking order from the bottom (M6).
const KINDS: [&str; 4] = ["close", "failed", "ask", "blocked"];

fn summary(ps: &[Value], (start, end): (i64, i64)) -> Summary {
    let mut seen = std::collections::HashMap::new();
    for p in ps {
        for t in p["tasks"].as_array().into_iter().flatten() {
            seen.insert(format!("{}/{}", p["issues"].as_str().unwrap_or(""), t["n"]), t["state"].as_str().unwrap_or("").to_string());
        }
    }
    let n = |want: &[&str]| seen.values().filter(|s| want.contains(&s.as_str())).count();
    let mut bins = vec![[0; 4]; 12];
    let mut ev = std::collections::HashSet::new();
    for p in ps {
        for e in p["events"].as_array().into_iter().flatten() {
            let (t, kind) = (e[0].as_i64().unwrap_or(0), e[1].as_str().unwrap_or(""));
            let Some(k) = KINDS.iter().position(|x| *x == kind) else { continue };
            if t >= start && t < end && ev.insert(format!("{} {} {kind}", p["issues"], e[2])) {
                bins[(((t - start) * 12 / (end - start)) as usize).min(11)][k] += 1;
            }
        }
    }
    let since = ps.iter().filter_map(|p| p["since"].as_i64()).max();
    Summary { counts: [n(&["doing", "review"]), n(&["ask"]), n(&["ready"])], run: ps.iter().any(|p| p["focus"].as_array().is_some_and(|f| !f.is_empty())), bins, since }
}

pub fn run(install: PathBuf, snapshot: Option<PathBuf>) -> Result<(), String> {
    if snapshot.is_none() && !alone(&install) {
        return Err("taskq tray already runs on this host".into());
    }
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        // menu bar only: no Dock icon, no app menu
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { rect, button_state: MouseButtonState::Up, .. } = e {
            let _ = proxy.send_event(Ev::Click(rect));
        }
    }));
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        while proxy.send_event(Ev::Tick).is_ok() {
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let proxy = event_loop.create_proxy();
    let mut settings = Settings::load(&install);
    let mut ui: Option<(Window, WebView)> = None;
    let mut tray: Option<TrayIcon> = None;
    let (mut shown, mut icon_sig, mut dark) = (String::new(), String::new(), true);
    let mut hidden_at = Instant::now() - Duration::from_secs(1);
    let mut anchor: Option<Rect> = None;
    let mut step = 0usize;
    let mut adding = Value::Null;
    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        let push = |ui: &Option<(Window, WebView)>, st: &Value| {
            if let Some((_, wv)) = ui {
                let _ = wv.evaluate_script(&format!("taskq.receive({st})"));
            }
        };
        match event {
            Event::NewEvents(StartCause::Init) => {
                let b = WindowBuilder::new().with_title("taskq").with_decorations(false).with_resizable(false).with_always_on_top(true)
                    .with_visible(snapshot.is_some()).with_inner_size(LogicalSize::new(320.0, 400.0)).with_position(tao::dpi::LogicalPosition::new(40.0, 40.0));
                #[cfg(windows)]
                let b = tao::platform::windows::WindowBuilderExtWindows::with_skip_taskbar(b, true);
                let p = proxy.clone();
                let built = b.build(target).map_err(|e| e.to_string()).and_then(|w| {
                    // only our page: a link goes to the browser through the bridge
                    WebViewBuilder::new().with_html(HTML).with_navigation_handler(|u| !u.starts_with("http"))
                        .with_ipc_handler(move |r| {
                            let _ = p.send_event(Ev::Ipc(r.into_body()));
                        })
                        .build(&w).map(|v| (w, v)).map_err(|e| e.to_string())
                });
                match built {
                    Ok(x) => {
                        dark = is_dark(&x.0);
                        ui = Some(x);
                    }
                    Err(e) => {
                        eprintln!("taskq tray: {e}");
                        *flow = ControlFlow::Exit;
                        return;
                    }
                }
                if snapshot.is_some() {
                    return;
                }
                // macOS needs a running event loop before the icon exists
                match TrayIconBuilder::new().with_icon(rgba_icon(&compact(false, dark))).with_tooltip("taskq").build() {
                    Ok(t) => tray = Some(t),
                    Err(e) => {
                        eprintln!("taskq tray: {e}");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::Focused(false), .. } if snapshot.is_none() => {
                if let Some((w, _)) = &ui {
                    w.set_visible(false);
                    hidden_at = Instant::now();
                }
            }
            Event::WindowEvent { event: WindowEvent::ThemeChanged(_), .. } => {
                if let Some((w, _)) = &ui {
                    dark = is_dark(w);
                    icon_sig.clear();
                    shown.clear();
                }
            }
            Event::UserEvent(Ev::Tick) if snapshot.is_none() => {
                // another version became current: exit, a service starts the current one (§ 4)
                if !alone(&install) || crate::platform::tool(&install).current().is_some_and(|v| v != crate::platform::VERSION) {
                    *flow = ControlFlow::Exit;
                    return;
                }
                let _ = fs::write(install.join("tray"), std::process::id().to_string());
                let st = state(&install, &settings, dark, None, &adding);
                refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);
                let sig = st.to_string().replace(&format!("\"now\":{}", st["now"]), "");
                if sig != shown {
                    shown = sig;
                    push(&ui, &st);
                }
            }
            Event::UserEvent(Ev::Click(rect)) => {
                let Some((w, _)) = &ui else { return };
                // a click on the icon while open first blurs the popover: that click only closes it
                if w.is_visible() || hidden_at.elapsed() < Duration::from_millis(300) {
                    w.set_visible(false);
                    return;
                }
                anchor = Some(rect);
                place(w, rect);
                w.set_visible(true);
                w.set_focus();
            }
            Event::UserEvent(Ev::Ipc(m)) => {
                let Ok(m) = serde_json::from_str::<Value>(&m) else { return };
                let Some((w, _)) = &ui else { return };
                match m["a"].as_str().unwrap_or("") {
                    "ready" => {
                        if snapshot.is_some() {
                            let _ = proxy.send_event(Ev::Shot);
                            return;
                        }
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "size" => {
                        let max = w.current_monitor().map_or(800.0, |m| m.size().to_logical::<f64>(m.scale_factor()).height) * if snapshot.is_some() { 0.95 } else { 0.8 };
                        w.set_inner_size(LogicalSize::new(m["w"].as_f64().unwrap_or(320.0), m["h"].as_f64().unwrap_or(400.0).min(max)));
                        if let Some(r) = anchor {
                            place(w, r);
                        }
                    }
                    "hide" => w.set_visible(false),
                    "resources" => {
                        std::thread::spawn(resources);
                    }
                    "open" => {
                        if let Some(u) = m["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                            open(u);
                        }
                    }
                    "set" => {
                        settings.set(&m);
                        settings.save(&install);
                        let st = state(&install, &settings, dark, None, &adding);
                        refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);
                        push(&ui, &st);
                    }
                    // `Edit limits` after the page's confirm: the same write as `service capacity` (§ 5), this host only
                    "capacity" => {
                        let root = m["root"].as_str().unwrap_or("");
                        if roots(&install).iter().any(|r| r == root) {
                            let msg = capacity(Path::new(root), &m);
                            push(&ui, &json!({"capmsg": {"root": root, "text": msg}}));
                        }
                    }
                    // Add project (§ 4): the folder comes only from the OS picker, never from the page
                    "add" => {
                        adding = json!({"state": "picking"});
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                        let p = proxy.clone();
                        std::thread::spawn(move || {
                            let v = pick().map_or(Value::Null, |d| classify(&d, selftest));
                            let _ = p.send_event(Ev::Added(v));
                        });
                    }
                    "add-cancel" => {
                        adding = Value::Null;
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "add-install" if adding["kind"] == "taskq" && adding["state"] == "picked" => {
                        adding["state"] = json!("installing");
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                        let (p, a, inst) = (proxy.clone(), adding.clone(), install.clone());
                        std::thread::spawn(move || {
                            let _ = p.send_event(Ev::Added(install_service(&inst, a)));
                        });
                    }
                    "add-setup" if matches!(adding["kind"].as_str(), Some("new" | "plain")) => {
                        let text = format!("Set up {} as a taskq project", adding["name"].as_str().unwrap_or("this folder"));
                        ai(&install, "ai-send", &json!({"text": text, "onboard": adding["path"]}));
                        adding = Value::Null;
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a if a.starts_with("ai-") => {
                        let mut m = m.clone();
                        m["onboard"] = Value::Null;
                        ai(&install, a, &m);
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a => {
                        // only a registered project (§ 4): the page never names a path of its own
                        let root = m["root"].as_str().unwrap_or("");
                        if roots(&install).iter().any(|r| r == root) {
                            act(a, Path::new(root), m["label"].as_str());
                        }
                    }
                }
            }
            Event::UserEvent(Ev::Added(v)) => {
                adding = v;
                push(&ui, &state(&install, &settings, dark, None, &adding));
                if let (Some((w, _)), false) = (&ui, adding.is_null()) {
                    w.set_visible(true);
                    w.set_focus();
                }
            }
            // snapshot mode: per step push a state, wait for layout, capture, next
            Event::UserEvent(Ev::Shot) => {
                let dir = snapshot.clone().unwrap_or_default();
                let steps = shots(&install);
                if step == 0 {
                    let _ = fs::create_dir_all(&dir);
                    let st = state(&install, &settings, dark, None, &adding);
                    let s = summary(st["projects"].as_array().map_or(&[][..], |v| v), settings.range(now()));
                    for (theme, d) in [("dark", true), ("light", false)] {
                        for (mode, run) in [("run", true), ("pause", false)] {
                            let _ = fs::write(dir.join(format!("icon-compact-{mode}-{theme}.png")), png(&compact(run, d)));
                            let _ = fs::write(dir.join(format!("icon-summary-{mode}-{theme}.png")), png(&wide(&Summary { run, ..s.clone() }, d, 1.0)));
                        }
                    }
                }
                let Some((theme, page, font, script)) = steps.get(step).cloned() else {
                    println!("snapshots in {}", dir.display());
                    *flow = ControlFlow::Exit;
                    return;
                };
                let mut s = settings.clone();
                s.font = font;
                push(&ui, &state(&install, &s, theme == "dark", Some((theme, &page)), &Value::Null));
                if let (Some((_, wv)), false) = (&ui, script.is_empty()) {
                    let _ = wv.evaluate_script(script);
                }
                let p = proxy.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(800));
                    let _ = p.send_event(Ev::Shot2);
                });
            }
            Event::UserEvent(Ev::Shot2) => {
                let Some((_, wv)) = &ui else { return };
                let dir = snapshot.clone().unwrap_or_default();
                let p = proxy.clone();
                let name = format!("popover-{}.png", shot_name(&install, step));
                step += 1;
                capture(wv, dir.join(name), move || {
                    let _ = p.send_event(Ev::Shot);
                });
            }
            _ => {}
        }
    })
}

/// The `actions` snapshot: a custom range with start >= end, then `Pause all…`, both before acting (menu.md M5, M6).
const ACTIONS: &str = "(()=>{const q=s=>document.querySelector(s),put=(s,v)=>{const i=q(s);i.value=v;i.dispatchEvent(new Event('input',{bubbles:true}));};
q('[data-v=custom]').click();put('#hs','2026-10-13T10:00');put('#he','2026-10-13T09:00');q('[data-a=range]').click();q('[data-a=confirm]').click();})()";

/// Snapshot steps: theme, page (`home` or a project root), text size, a script run on the page.
fn shots(install: &Path) -> Vec<(&'static str, String, u64, &'static str)> {
    let first = roots(install).into_iter().next();
    ["dark", "light"].iter().flat_map(|t| {
        let mut v = vec![(*t, "home".to_string(), 16, ""), (*t, "home".to_string(), 20, ""), (*t, "home".to_string(), 16, ACTIONS)];
        v.extend(first.clone().map(|r| (*t, r, 16, "")));
        v
    }).collect()
}

fn shot_name(install: &Path, step: usize) -> String {
    shots(install).get(step).map_or(step.to_string(), |(t, p, f, js)| format!("{t}-{}{}-{f}", if p == "home" { "home" } else { "project" }, if js.is_empty() { "" } else { "-actions" }))
}

fn state(install: &Path, s: &Settings, dark: bool, snap: Option<(&str, &str)>, add: &Value) -> Value {
    let ps: Vec<Value> = roots(install).iter().map(|r| project(r)).collect();
    let (start, end) = s.range(now());
    let sum = summary(&ps, (start, end));
    let url = |rgba: &Rgba| format!("data:image/png;base64,{}", base64(&png(rgba)));
    let mut st = json!({"platform": if cfg!(windows) { "windows" } else { "macos" }, "version": crate::platform::VERSION,
        "host": ps.iter().find_map(|p| p["host"].as_str()).unwrap_or("?"), "view": s.view, "font": s.font, "period": s.period,
        "now": now(), "projects": ps, "agents": agents(&ps), "ai": {"target": target(install), "request": newest(install), "draft": requests_file(install)["draft"], "history": requests_file(install)["history"]}, "add": add, "history": {"start": start, "end": end, "tz": s.tz, "bins": sum.bins, "since": sum.since}});
    if cfg!(target_os = "macos") {
        st["icons"] = json!({"icon": url(&compact(sum.run, dark)), "full": url(&wide(&sum, dark, 1.0))});
    }
    if let Some((theme, page)) = snap {
        st["theme"] = json!(theme);
        st["page"] = json!(page);
    }
    st
}

/// Host-level `Agents` (§ 4): the rows of the freshest probe in any `status.json`, as menu lines; null when none has one.
fn agents(ps: &[Value]) -> Value {
    let Some(a) = ps.iter().map(|p| &p["agents"]).filter(|a| a["at"].is_u64()).max_by_key(|a| a["at"].as_u64()) else { return Value::Null };
    json!({"at": a["at"], "rows": a["rows"].as_array().into_iter().flatten().map(crate::runtime::agent_text).collect::<Vec<_>>()})
}

fn refresh_icon(tray: &Option<TrayIcon>, st: &Value, s: &Settings, dark: bool, sig: &mut String) {
    let Some(t) = tray else { return };
    let sum = summary(st["projects"].as_array().map_or(&[][..], |v| v), s.range(now()));
    // Windows shows only the compact q (menu.md M1)
    let full = !cfg!(windows) && s.view == "full";
    let k = if full { menu_bar_scale() } else { 1.0 };
    let new = format!("{full} {dark} {k} {} {:?} {:?}", sum.run, sum.counts, if full { sum.bins.clone() } else { vec![] });
    if *sig == new {
        return;
    }
    *sig = new;
    let _ = t.set_icon(Some(rgba_icon(&if full { wide(&sum, dark, k) } else { compact(sum.run, dark) })));
    if k > 1.0 {
        fit_status_image(k);
    }
    let [w, a, r] = sum.counts;
    let _ = t.set_tooltip(Some(format!("taskq: in work {w} · waiting for answer {a} · ready {r}{}", if sum.run { "" } else { " · paused" })));
}

/// Put the popover under a menu-bar icon or above a taskbar icon, inside the icon's monitor.
fn place(w: &Window, r: Rect) {
    let (ix, iy) = (r.position.x, r.position.y);
    let m = w.available_monitors().find(|m| {
        let (p, s) = (m.position(), m.size());
        ix >= p.x as f64 && ix < (p.x + s.width as i32) as f64 && iy >= p.y as f64 && iy < (p.y + s.height as i32) as f64
    });
    let (mp, ms) = m.map_or(((0, 0), (1920, 1080)), |m| ((m.position().x, m.position().y), (m.size().width as i32, m.size().height as i32)));
    let sz = w.outer_size();
    let (ww, wh) = (sz.width as i32, sz.height as i32);
    let x = (ix as i32 + r.size.width as i32 / 2 - ww / 2).clamp(mp.0, (mp.0 + ms.0 - ww).max(mp.0));
    let y = if (iy as i32) < mp.1 + ms.1 / 2 { iy as i32 + r.size.height as i32 + 4 } else { iy as i32 - wh - 4 };
    w.set_outer_position(PhysicalPosition::new(x, y.clamp(mp.1, (mp.1 + ms.1 - wh).max(mp.1))));
}

fn act(action: &str, root: &Path, label: Option<&str>) {
    let d = crate::dot(root);
    match action {
        "pause" => { let _ = crate::pause(root); }
        "resume" => {
            let _ = crate::resume(root);
        }
        // a label of the project's open tasks, never `all` (§ 4)
        "focus" => {
            if let Some(l) = label.filter(|l| crate::status_labels(root).iter().any(|x| x == l)) {
                let _ = crate::set_focus(root, l, None);
            }
        }
        // `Update now` (menu.md M9): the service checks at once (§ 4 step 7); the page waits for the version in status.json
        "update" => {
            let _ = fs::write(d.join("update"), "");
            crate::poke(root);
        }
        "restart" => {
            let _ = fs::write(d.join("restart"), "");
            crate::poke(root);
        }
        "log" => {
            let s: Value = fs::read_to_string(d.join("status.json")).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default();
            if let Some(l) = s["log"].as_str() {
                open(l);
            }
        }
        "folder" => open(&root.display().to_string()),
        // a separate process: on macOS `launchctl bootout` kills the service's process group, this tray included
        "stop" | "uninstall" => {
            let Ok(me) = std::env::current_exe() else {
                return;
            };
            let mut c = std::process::Command::new(me);
            c.args(["service", action])
                .current_dir(root)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            let _ = crate::platform::quiet(&mut c).spawn();
        }
        _ => {}
    }
}

/// `Resources` (menu.md M5): btop in its own OS terminal window, installed first with the OS package manager when
/// missing, its output in that window; no package manager: one line saying so.
const BTOP_SH: &str = r#"#!/bin/sh
printf '\033]0;Resources\007\033[8;30;110t'
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
if ! command -v btop >/dev/null; then
  if ! command -v brew >/dev/null; then echo "btop is not installed and Homebrew was not found: install btop, then try again."; read _; exit 1; fi
  echo "Installing btop: brew install btop"
  brew install btop || { echo "brew install btop failed (see above)."; read _; exit 1; }
fi
exec btop
"#;
const BTOP_PS1: &str = r#"$Host.UI.RawUI.WindowTitle = 'Resources'
if (-not (Get-Command btop -ErrorAction SilentlyContinue)) {
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { 'btop4win is not installed and winget was not found: install btop4win, then try again.'; Read-Host; exit 1 }
  'Installing btop: winget install aristocratos.btop4win'
  winget install -e --id aristocratos.btop4win --accept-source-agreements --accept-package-agreements
  $env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
  if (-not (Get-Command btop -ErrorAction SilentlyContinue)) { 'winget install aristocratos.btop4win failed (see above).'; Read-Host; exit 1 }
}
btop
"#;

fn resources() {
    let file = std::env::temp_dir().join(if cfg!(windows) { "taskq-resources.ps1" } else { "taskq-resources.command" });
    if fs::write(&file, if cfg!(windows) { BTOP_PS1 } else { BTOP_SH }).is_err() {
        return;
    }
    // not `quiet`: the terminal window is the point; waited on its own thread, no zombie
    let _ = if cfg!(windows) {
        std::process::Command::new("conhost.exe").args(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file).status()
    } else {
        #[cfg(unix)]
        let _ = fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        std::process::Command::new("open").args(["-a", "Terminal"]).arg(&file).status()
    };
}

/// The OS folder picker; None on cancel.
fn pick() -> Option<PathBuf> {
    let mut c = if cfg!(windows) {
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-STA", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Add project'; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }"]);
        c
    } else {
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", "activate", "-e", "POSIX path of (choose folder with prompt \"Add project\")"]);
        c
    };
    let o = crate::platform::quiet(&mut c).output().ok().filter(|o| o.status.success())?;
    let p = String::from_utf8_lossy(&o.stdout).trim().trim_end_matches(['/', '\\']).to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// `taskq selftest` in `dir`: its first error line on failure.
fn selftest(dir: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let o = crate::platform::quiet(std::process::Command::new(me).arg("selftest").current_dir(dir).env("TASKQ_ROOT", dir)).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    if o.status.success() { Ok(()) } else { Err(text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("selftest failed").chars().take(160).collect()) }
}

/// The four Add project cases (menu.md M10): `taskq.json` with a passing `selftest` (taskq), with a failing one (old,
/// owner's marker), else an empty folder or a Git root (new) or files without Git (plain: not a repository).
fn classify(dir: &Path, selftest: impl Fn(&Path) -> Result<(), String>) -> Value {
    let (kind, reason) = if dir.join("taskq.json").is_file() {
        selftest(dir).map_or_else(|e| ("old", e), |_| ("taskq", String::new()))
    } else if dir.join(".git").exists() || fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
        ("new", String::new())
    } else {
        ("plain", String::new())
    };
    json!({"state": "picked", "path": dir, "name": crate::project_name(dir), "kind": kind, "reason": reason})
}

/// `service install` in the picked taskq project after the page's confirm; installed only when `<install>/projects`
/// lists it (R12).
fn install_service(install: &Path, mut a: Value) -> Value {
    let dir = PathBuf::from(a["path"].as_str().unwrap_or(""));
    let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|me| {
        let mut c = std::process::Command::new(me);
        c.args(["service", "install"]).current_dir(&dir).env("TASKQ_ROOT", &dir).stdin(std::process::Stdio::null());
        // a separate process group: launchctl must not take the tray with it
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let o = crate::platform::quiet(&mut c).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
        if !o.status.success() {
            return Err(text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("service install failed").to_string());
        }
        if roots(install).iter().any(|r| Path::new(r) == dir) { Ok(()) } else { Err("installed, but this host's projects list does not show it".into()) }
    });
    a["state"] = json!(if r.is_ok() { "installed" } else { "install failed" });
    a["reason"] = json!(r.err().unwrap_or_default());
    a
}

/// Write the page's capacity edit for this host; the outcome line for the page.
fn capacity(root: &Path, m: &Value) -> String {
    let set = &m["set"];
    let mut e = crate::CapEdit { gate: set["enabled"].as_bool(), ..Default::default() };
    let num = |k: &str| set[k].as_u64().map(|n| u32::try_from(n).map_err(|_| format!("{k}: not an integer 0..{}", u32::MAX))).transpose();
    let r = (|| {
        e.cpu_max = num("cpu_max")?;
        e.memory_min = num("memory_min")?;
        for (rt, n) in set["limits"].as_object().into_iter().flatten() {
            e.limits.insert(rt.clone(), n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("{rt}: not an integer 0..{}", u32::MAX))?);
        }
        let me = crate::load(root)?.0.me;
        crate::write_capacity(root, &me, &e, Some(&m["expect"]))
    })();
    match r {
        Ok((old, new)) => format!("Saved: {}", crate::cap_changes(&old, &new)),
        Err(e) => format!("Error: {e}"),
    }
}

fn open(arg: &str) {
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    let _ = crate::platform::quiet(std::process::Command::new(opener).arg(arg)).spawn();
}

/// The menu bar's theme: macOS follows the app appearance; the Windows taskbar has its own setting.
fn is_dark(_w: &Window) -> bool {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("reg");
        c.args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "SystemUsesLightTheme"]);
        let out = crate::platform::quiet(&mut c).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        !out.contains("0x1")
    }
    #[cfg(not(windows))]
    {
        _w.theme() == tao::window::Theme::Dark
    }
}

// ---------- icons (docs/design/icon-studies.html) ----------

/// An RGBA image at 2 px per point: the menu bar is 22 pt high (macOS); Windows scales the square one.
struct Rgba {
    w: u32,
    h: u32,
    /// pixels per point of the 22 pt drawing: 2 (Retina) times the menu-bar scale `k`
    s: f32,
    px: Vec<u8>,
}

impl Rgba {
    /// A 22 pt tall canvas; `k` > 1 scales the drawing up to a taller menu bar.
    fn new(w_pt: f32, k: f32) -> Rgba {
        let s = 2.0 * k;
        let (w, h) = ((w_pt * s).ceil() as u32, (22.0 * s).round() as u32);
        Rgba { w, h, s, px: vec![0; (w * h * 4) as usize] }
    }

    /// Paint `rgb` (alpha 0 clears) where `inside(x, y)` holds, in points, 4×4 samples per pixel.
    fn fill(&mut self, rgb: [u8; 4], inside: impl Fn(f32, f32) -> bool) {
        for y in 0..self.h {
            for x in 0..self.w {
                let mut hit = 0;
                for i in 0..16 {
                    let (sx, sy) = ((x as f32 + (i % 4) as f32 / 4.0 + 0.125) / self.s, (y as f32 + (i / 4) as f32 / 4.0 + 0.125) / self.s);
                    hit += inside(sx, sy) as u32;
                }
                if hit == 0 {
                    continue;
                }
                let a = hit as f32 / 16.0 * rgb[3] as f32 / 255.0;
                let o = ((y * self.w + x) * 4) as usize;
                let da = self.px[o + 3] as f32 / 255.0;
                if rgb[3] == 0 {
                    self.px[o + 3] = (da * (1.0 - hit as f32 / 16.0) * 255.0) as u8;
                    continue;
                }
                let oa = a + da * (1.0 - a);
                for c in 0..3 {
                    self.px[o + c] = ((rgb[c] as f32 * a + self.px[o + c] as f32 * da * (1.0 - a)) / oa) as u8;
                }
                self.px[o + 3] = (oa * 255.0) as u8;
            }
        }
    }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| x >= x0 && x < x1 && y >= y0 && y < y1
}

fn rrect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let (dx, dy) = ((x0 + r - x).max(x - x1 + r).max(0.0), (y0 + r - y).max(y - y1 + r).max(0.0));
        x >= x0 && x < x1 && y >= y0 && y < y1 && dx * dx + dy * dy <= r * r
    }
}

struct Palette {
    ink: [u8; 4],
    work: [u8; 4],
    ask: [u8; 4],
    muted: [u8; 4],
    failed: [u8; 4],
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette { ink: [236, 239, 242, 255], work: [119, 205, 179, 255], ask: [237, 185, 147, 255], muted: [180, 189, 200, 255], failed: [239, 137, 150, 255] }
    } else {
        Palette { ink: [37, 41, 48, 255], work: [35, 120, 98, 255], ask: [163, 92, 41, 255], muted: [98, 107, 119, 255], failed: [174, 56, 76, 255] }
    }
}

/// The angular q (18 pt, stroke 1.8, tail bottom right) at `(x0, y0)` with ▶ (admits) or Ⅱ inside.
fn draw_q(img: &mut Rgba, x0: f32, y0: f32, run: bool, ink: [u8; 4], p: &Palette) {
    let (x1, y1) = (x0 + 18.0, y0 + 18.0);
    let (outer, inner) = (rrect(x0, y0, x1, y1, 1.8), rrect(x0 + 1.8, y0 + 1.8, x1 - 1.8, y1 - 1.8, 0.4));
    img.fill(ink, |x, y| outer(x, y) && !inner(x, y));
    img.fill(ink, rect(x1 - 1.8, y1 - 1.0, x1, y1 + 3.6));
    let (cx, cy) = (x0 + 9.0, y0 + 9.0);
    if run {
        let (l, t) = (cx - 4.05 + 0.9, cy - 4.5);
        img.fill(p.work, move |x, y| x >= l && y >= t + (x - l) * 4.5 / 8.1 && y <= t + 9.0 - (x - l) * 4.5 / 8.1);
    } else {
        img.fill(p.muted, rect(cx - 4.5, cy - 4.5, cx - 1.8, cy + 4.5));
        img.fill(p.muted, rect(cx + 1.8, cy - 4.5, cx + 4.5, cy + 4.5));
    }
}

fn compact(run: bool, dark: bool) -> Rgba {
    let p = palette(dark);
    let mut img = Rgba::new(22.0, 1.0);
    draw_q(&mut img, 2.0, 0.2, run, p.ink, &p);
    img
}

/// Segments a–g of a 6×9 pt seven-segment digit at the origin, stroke 1.6: angular like the q.
fn segments(d: u32) -> impl Iterator<Item = (f32, f32, f32, f32)> {
    let (w, h, t) = (6.0, 9.0, 1.6);
    let seg = [
        (0.0, 0.0, w, t),
        (w - t, 0.0, w, (h + t) / 2.0),
        (w - t, (h - t) / 2.0, w, h),
        (0.0, h - t, w, h),
        (0.0, (h - t) / 2.0, t, h),
        (0.0, 0.0, t, (h + t) / 2.0),
        (0.0, (h - t) / 2.0, w, (h + t) / 2.0),
    ];
    let on = ["abcdef", "bc", "abged", "abgcd", "fgbc", "afgcd", "afgedc", "abc", "abcdefg", "abcdfg"][d as usize % 10];
    on.bytes().map(move |ch| seg[(ch - b'a') as usize])
}

fn digit(img: &mut Rgba, x: f32, y: f32, d: u32, c: [u8; 4]) {
    for (a, b, cc, dd) in segments(d) {
        img.fill(c, rect(x + a, y + b, x + cc, y + dd));
    }
}

/// Ink bounds `(left, right)` of `n` drawn from x = 0 in 7 pt digit cells: a 1 has ink only in its right 1.6 pt.
fn number_ink(n: usize) -> (f32, f32) {
    let ds: Vec<u32> = n.to_string().bytes().map(|ch| (ch - b'0') as u32).collect();
    let left = segments(ds[0]).map(|s| s.0).fold(f32::MAX, f32::min);
    let right = segments(ds[ds.len() - 1]).map(|s| s.2).fold(0.0, f32::max);
    (left, (ds.len() - 1) as f32 * 7.0 + right)
}

fn number_width(n: usize) -> f32 {
    let (l, r) = number_ink(n);
    r - l
}

/// macOS summary (menu.md M1, single ribbon): in work left, `ask` centered, ready right in 12 pt over 7 pt stacked
/// close/failed bars on one peak scale. All paused: the same in gray with the compact q and Ⅱ centered.
/// The summary ribbon (menu.md M1): three numbers 4 pt apart over the full-width cadence bars,
/// drawn at 22 pt and scaled by `k` to the menu-bar height.
fn wide(s: &Summary, dark: bool, k: f32) -> Rgba {
    let p = palette(dark);
    let widths = s.counts.map(number_width);
    let w = widths.iter().sum::<f32>() + 8.0;
    let mut img = Rgba::new(w, k);
    let gray = |c: [u8; 4]| if s.run { c } else { p.muted };
    for (i, (n, c)) in s.counts.iter().zip([p.work, p.ask, p.muted]).enumerate() {
        // align by ink, not by cell (menu.md M1)
        let (l, r) = number_ink(*n);
        let x = [-l, (w - l - r) / 2.0, w - r][i];
        for (k, ch) in n.to_string().bytes().enumerate() {
            digit(&mut img, x + k as f32 * 7.0, 1.5, (ch - b'0') as u32, gray(c));
        }
    }
    // ponytail: the icon keeps close/failed only (§ 4); the popover stacks all four kinds
    let peak = s.bins.iter().map(|b| b[0] + b[1]).max().unwrap_or(0).max(1) as f32;
    let bw = (w + 1.0) / s.bins.len() as f32;
    for (i, [close, failed, ..]) in s.bins.iter().enumerate() {
        let (x, base) = (i as f32 * bw, 22.0);
        let (gh, rh) = (*close as f32 * 7.0 / peak, *failed as f32 * 7.0 / peak);
        img.fill(gray(p.work), rect(x, base - gh, x + bw - 1.0, base));
        img.fill(gray(p.failed), rect(x, base - gh - rh, x + bw - 1.0, base - gh));
    }
    if !s.run {
        let x0 = (w - 18.0) / 2.0;
        img.fill([0, 0, 0, 0], rect(x0 - 1.0, -1.0, x0 + 19.0, 23.0));
        draw_q(&mut img, x0, 0.2, false, p.muted, &p);
    }
    img
}

fn rgba_icon(i: &Rgba) -> Icon {
    Icon::from_rgba(i.px.clone(), i.w, i.h).expect("rgba icon")
}

/// Call `f` with the status item's window and button (tray-icon keeps them private).
#[cfg(target_os = "macos")]
fn with_status_button(mut f: impl FnMut(&objc2_app_kit::NSWindow, &objc2_app_kit::NSButton)) {
    use objc2_app_kit::{NSApplication, NSButton, NSView, NSWindow};
    use objc2_foundation::MainThreadMarker;
    fn walk(w: &NSWindow, v: &NSView, f: &mut impl FnMut(&NSWindow, &NSButton)) -> bool {
        if let Some(b) = v.downcast_ref::<NSButton>() {
            f(w, b);
            return true;
        }
        v.subviews().iter().any(|s| walk(w, &s, f))
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    for w in NSApplication::sharedApplication(mtm).windows().iter() {
        if w.class().name().to_str() == Ok("NSStatusBarWindow") && w.contentView().is_some_and(|v| walk(&w, &v, &mut f)) {
            return;
        }
    }
}

/// Menu-bar height over the 22 pt the drawing assumes: 1.5 on notch Macs, 1 elsewhere.
#[cfg(target_os = "macos")]
fn menu_bar_scale() -> f32 {
    let mut bar = 22.0f32;
    // 0 until the item is placed: the next tick re-renders (`k` is in the icon signature)
    with_status_button(|w, _| bar = w.frame().size.height as f32);
    if (22.0..=44.0).contains(&bar) { bar / 22.0 } else { 1.0 }
}

#[cfg(not(target_os = "macos"))]
fn menu_bar_scale() -> f32 {
    1.0
}

#[cfg(not(target_os = "macos"))]
fn fit_status_image(_: f32) {}

/// tray-icon caps the status-item image at 22 pt; grow it back to the full menu-bar height.
#[cfg(target_os = "macos")]
fn fit_status_image(k: f32) {
    use objc2_foundation::NSSize;
    with_status_button(|_, b| {
        let Some(img) = b.image() else { return };
        let sz = img.size();
        if (sz.height - 22.0).abs() < 0.5 {
            img.setSize(NSSize::new(sz.width * k as f64, 22.0 * k as f64));
            // the button takes its height from a newly set image, not from a resized one
            b.setImage(None);
            b.setImage(Some(&img));
        }
    });
}

fn png(i: &Rgba) -> Vec<u8> {
    let mut out = vec![];
    let mut e = ::png::Encoder::new(&mut out, i.w, i.h);
    e.set_color(::png::ColorType::Rgba);
    if let Ok(mut w) = e.write_header() {
        let _ = w.write_image_data(&i.px);
    }
    out
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            s.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

// ---------- popover snapshot (`taskq tray --snapshot`): the webview renders its own page ----------

#[cfg(target_os = "macos")]
fn capture(wv: &WebView, path: PathBuf, done: impl Fn() + 'static) {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use wry::WebViewExtMacOS;
    let block = block2::RcBlock::new(move |img: *mut NSImage, _: *mut NSError| {
        // SAFETY: WebKit passes a valid image or null
        if let Some(img) = unsafe { img.as_ref() } {
            let png = img.TIFFRepresentation().and_then(|t| NSBitmapImageRep::imageRepWithData(&t))
                .and_then(|r| unsafe { r.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) });
            if let Some(p) = png {
                let _ = fs::write(&path, p.to_vec());
            }
        }
        done();
    });
    // SAFETY: main thread, a live webview
    unsafe { wv.webview().takeSnapshotWithConfiguration_completionHandler(None, &block) };
}

#[cfg(windows)]
fn capture(wv: &WebView, path: PathBuf, done: impl Fn() + 'static) {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::System::Com::{STGM_CREATE, STGM_WRITE};
    use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
    use wry::WebViewExtWindows;
    let p = windows::core::HSTRING::from(path.as_os_str());
    // SAFETY: a file stream handed to WebView2, released by the handler after the capture
    let Ok(stream) = (unsafe { SHCreateStreamOnFileEx(&p, (STGM_CREATE | STGM_WRITE).0, FILE_ATTRIBUTE_NORMAL.0, true, None) }) else {
        return done();
    };
    let done = std::rc::Rc::new(done);
    let (mut keep, d) = (Some(stream.clone()), done.clone());
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |_| {
        keep.take();
        d();
        Ok(())
    }));
    // SAFETY: UI thread, a live webview
    if unsafe { wv.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) }.is_err() {
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clarify_thread_is_oldest_first_and_stops_at_missing_or_cycle() {
        let install = std::env::temp_dir().join(format!("taskq-thread-{}", std::process::id()));
        let root = install.join("project");
        fs::create_dir_all(crate::operator::dir(&root)).unwrap();
        fs::write(install.join("projects"), root.to_str().unwrap()).unwrap();
        let put = |id: &str, after: &str| {
            fs::write(crate::operator::dir(&root).join(format!("{id}.json")), json!({"text": id, "after": after}).to_string()).unwrap();
            fs::write(crate::operator::answer_path(&root, id), json!({"state": "clarify", "text": format!("question {id}")}).to_string()).unwrap();
        };
        put("1", ""); put("2", "1"); put("3", "2");
        let q = newest(&install);
        assert_eq!(q["id"], "3");
        assert_eq!(q["chain"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(q["answer"]["text"], "question 3");
        put("1", "3");
        assert_eq!(newest(&install)["chain"].as_array().unwrap().len(), 3);
        put("2", "0");
        assert_eq!(newest(&install)["chain"].as_array().unwrap().len(), 2);
        fs::remove_dir_all(install).unwrap();
    }

    fn has(i: &Rgba, c: [u8; 4]) -> bool {
        i.px.chunks(4).any(|p| p[3] == 255 && p[..3] == c[..3])
    }

    #[test]
    fn compact_q_shows_run_or_pause() {
        let (p, run, pause) = (palette(true), compact(true, true), compact(false, true));
        assert_eq!((run.w, run.h), (44, 44));
        assert!(has(&run, p.work) && !has(&run, p.muted));
        assert!(has(&pause, p.muted) && !has(&pause, p.work));
        assert!(has(&compact(true, false), palette(false).ink));
    }

    #[test]
    fn summary_widens_for_long_numbers_and_grays_when_paused() {
        let mut s = Summary { counts: [1, 0, 2], run: true, bins: vec![[1, 0, 0, 0]; 12], since: None };
        let pt = s.counts.map(number_width).iter().sum::<f32>() + 8.0;
        assert_eq!((wide(&s, true, 1.0).w, wide(&s, true, 1.0).h), ((pt * 2.0).ceil() as u32, 44), "three ink widths, two 4 pt gaps, at 2x");
        assert_eq!((wide(&s, true, 1.5).w, wide(&s, true, 1.5).h), ((pt * 3.0).ceil() as u32, 66), "a 33 pt menu bar scales the drawing");
        s.counts = [100, 12, 1000];
        assert!(wide(&s, true, 1.0).w > (pt * 2.0).ceil() as u32);
        assert_eq!(wide(&s, true, 1.0).h, 44);
        s.run = false;
        let p = palette(true);
        assert!(!has(&wide(&s, true, 1.0), p.work) && has(&wide(&s, true, 1.0), p.muted));
    }

    #[test]
    fn summary_counts_align_by_ink() {
        let p = palette(true);
        for counts in [[1, 1, 1], [1, 0, 2], [10, 1, 71]] {
            let img = wide(&Summary { counts, run: true, bins: vec![[0; 4]; 12], since: None }, true, 1.0);
            let ink = |x: u32, c: Option<[u8; 4]>| {
                (0..img.h).any(|y| {
                    let px = &img.px[((y * img.w + x) * 4) as usize..][..4];
                    px[3] > 0 && c.is_none_or(|c| (0..3).all(|k| px[k].abs_diff(c[k]) <= 1))
                })
            };
            assert!(ink(0, None) && ink(img.w - 1, None), "{counts:?}: outer counts touch both edges");
            let mid: Vec<u32> = (0..img.w).filter(|x| ink(*x, Some(p.ask))).collect();
            let center = (mid[0] + mid[mid.len() - 1] + 1) as f32 / 2.0;
            assert!((center - img.w as f32 / 2.0).abs() <= 1.0, "{counts:?}: middle ink center {center} of {}", img.w);
        }
    }

    #[test]
    fn summary_counts_one_task_per_board_and_issue() {
        let p = |issues: &str, state: &str, focus: Value| json!({"issues": issues, "focus": focus, "tasks": [{"n": 1, "state": state}], "events": [[95, "close", "7"], [-250, "failed", "8"]]});
        let ps = [p("https://github.com/o/a/issues", "ask", json!([])), p("https://github.com/o/a/issues", "ask", json!([])), p("https://github.com/o/b/issues", "doing", json!(["all"]))];
        let s = summary(&ps, (-200, 100));
        assert_eq!(s.counts, [1, 1, 0]);
        assert!(s.run && !summary(&ps[..2], (-200, 100)).run);
        assert_eq!(s.bins.iter().map(|b| b[0]).sum::<u32>(), 2, "same attempt on two boards counts twice, on one board once");
        assert_eq!(s.bins.iter().map(|b| b[1]).sum::<u32>(), 0, "outside the period");
    }

    #[test]
    fn settings_keep_only_known_values() {
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into() };
        s.set(&json!({"view": "full", "font": 18, "period": "1w"}));
        s.set(&json!({"view": "x", "font": 15, "period": "2y"}));
        s.set(&json!({"period": "custom", "start": 50, "end": 50, "tz": "UTC"}));
        assert!(s == Settings { view: "full".into(), font: 18, period: "1w".into(), start: 0, end: 0, tz: "local".into() });
        assert_eq!(s.range(1000), (1000 - 604800, 1000));
        s.set(&json!({"period": "custom", "start": 40, "end": 160, "tz": "UTC"}));
        assert_eq!((s.range(1000), s.tz.as_str()), ((40, 160), "UTC"));
    }

    #[test]
    fn history_bins_are_equal_oldest_left_on_one_scale() {
        // M-A5/M-A6 fixture: 3 of each kind in [0, 120); the first and last bins are not empty; unknown kinds drop
        let ev = json!([[0, "close", "1"], [5, "close", "2"], [119, "close", "3"], [0, "failed", "4"], [60, "failed", "5"], [119, "failed", "6"], [120, "failed", "7"], [-1, "close", "8"],
            [0, "ask", "9"], [30, "ask", "10"], [119, "ask", "11"], [10, "blocked", "12"], [60, "blocked", "13"], [119, "blocked", "14"], [50, "later", "15"]]);
        let p = json!([{"issues": "u", "tasks": [], "focus": ["all"], "events": ev, "since": -10}]);
        let s = summary(p.as_array().unwrap(), (0, 120));
        assert_eq!(s.bins.len(), 12);
        assert_eq!((s.bins[0], s.bins[6], s.bins[11]), ([2, 1, 1, 0], [0, 1, 0, 1], [1, 1, 1, 1]), "[start, end): 120 and -1 are out");
        assert_eq!((0..4).map(|k| s.bins.iter().map(|b| b[k]).sum::<u32>()).collect::<Vec<_>>(), [3, 3, 3, 3]);
        assert_eq!(s.since, Some(-10));
        // equal counts draw equal heights in the icon: as many green as red pixel rows in a column clear of the digits
        let one = Summary { counts: [0, 0, 0], run: true, bins: vec![[3, 3, 3, 3]; 12], since: None };
        let (img, pal) = (wide(&one, true, 1.0), palette(true));
        let col = |c: [u8; 4]| (0..img.h).filter(|y| img.px[((y * img.w + 14) * 4) as usize..][..3] == c[..3]).count();
        assert!(col(pal.work) > 0 && col(pal.work) == col(pal.failed));
        let ps = json!([{"issues": "u", "events": []}, {"issues": "v", "events": [], "since": 5}]);
        assert_eq!(summary(&ps.as_array().unwrap()[..1], (0, 1)).since, None, "no events cache: no since");
        assert_eq!(summary(ps.as_array().unwrap(), (0, 1)).since, Some(5));
    }

    #[test]
    fn history_keeps_20_distinct_newest_first() {
        let mut h = vec![];
        for i in 0..25 {
            h = remember(&h, &format!("q{i}"));
        }
        h = remember(&h, "q10");
        assert_eq!(h.len(), 20);
        assert_eq!((h[0].as_str(), h[1].as_str(), h.iter().filter(|x| x.as_str() == Some("q10")).count()), (Some("q10"), Some("q24"), 1));
    }

    #[test]
    fn agents_come_from_the_freshest_probe_or_are_unknown() {
        let row = |v: &str| json!({"runtime": "claude", "path": "/c", "version": v, "login": "not logged in", "account": null});
        let ps = [json!({"agents": {"at": 10, "rows": [row("1")]}}), json!({"agents": {"at": 20, "rows": [row("2")]}}), json!({})];
        assert_eq!(agents(&ps), json!({"at": 20, "rows": ["claude 2 · not logged in"]}));
        assert_eq!(agents(&ps[2..]), Value::Null);
    }

    #[test]
    fn add_project_has_four_cases() {
        let d = std::env::temp_dir().join(format!("taskq-add-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let kind = |st: &dyn Fn(&Path) -> Result<(), String>| { let v = classify(&d, st); (v["kind"].as_str().unwrap().to_string(), v["reason"].as_str().unwrap().to_string()) };
        let pass = |_: &Path| Ok(());
        assert_eq!(kind(&pass).0, "new", "empty folder");
        fs::write(d.join("notes.txt"), "x").unwrap();
        assert_eq!(kind(&pass).0, "plain", "files without Git: not a repository");
        fs::create_dir(d.join(".git")).unwrap();
        assert_eq!(kind(&pass).0, "new", "a Git root without taskq.json");
        fs::write(d.join("taskq.json"), "{}").unwrap();
        assert_eq!(kind(&pass).0, "taskq");
        assert_eq!(kind(&|_: &Path| Err("taskq.json: repo is required".into())), ("old".into(), "taskq.json: repo is required".into()), "the verified reason");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"f"), "Zg==");
    }
}
