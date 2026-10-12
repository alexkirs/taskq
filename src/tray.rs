//! `taskq tray` (taskq.md § 4): the host's one menu-bar / notification-area icon over the `<root>/.taskq/status.json`
//! of every project in `<install>/projects`. No board access and no token: actions go through the same files and
//! commands as the CLI.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// False when another tray wrote the heartbeat `<install>/tray` in the last 10 s (it ticks every 2 s): of two trays
/// started at once, the first to see the other's pid exits.
fn alone(install: &Path) -> bool {
    let hb = install.join("tray");
    let other = fs::read_to_string(&hb).ok().and_then(|p| p.trim().parse::<u32>().ok()).is_some_and(|p| p != std::process::id());
    !(other && crate::file_age(&hb).is_some_and(|a| a < 10))
}

pub fn run(install: PathBuf) -> Result<(), String> {
    if !alone(&install) {
        return Err("taskq tray already runs on this host".into());
    }
    // ticks come from a timer thread and menu clicks from the handler, both through the proxy
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Option<String>>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        // menu bar only: no Dock icon, no app menu
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = proxy.send_event(Some(e.id.0));
    }));
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        while proxy.send_event(None).is_ok() {
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let mut tray: Option<TrayIcon> = None;
    let mut shown = String::new();
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                // macOS needs a running event loop before the icon exists
                match TrayIconBuilder::new().with_icon(icon([128, 128, 128])).with_tooltip("taskq").build() {
                    Ok(t) => tray = Some(t),
                    Err(e) => {
                        eprintln!("taskq tray: {e}");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            Event::UserEvent(Some(id)) => act(&id),
            Event::UserEvent(None) => {
                if let Some(t) = &tray {
                    // another version became current: exit, a service starts the current one (§ 4)
                    if !alone(&install) || crate::platform::current(&install).is_some_and(|v| v != crate::platform::VERSION) {
                        *flow = ControlFlow::Exit;
                        return;
                    }
                    let _ = fs::write(install.join("tray"), std::process::id().to_string());
                    refresh(t, &install, &mut shown);
                }
            }
            _ => {}
        }
    })
}

struct Project {
    root: PathBuf,
    s: Value,
    alive: bool,
    tasks: Vec<(u64, String, String)>,
}

impl Project {
    fn read(root: &str) -> Project {
        let d = crate::dot(Path::new(root));
        let s: Value = fs::read_to_string(d.join("status.json")).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(Value::Null);
        let alive = crate::file_age(&d.join("status.json")).is_some_and(|a| a < 300);
        let tasks = s["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| (x["n"].as_u64().unwrap_or(0), x["title"].as_str().unwrap_or("").chars().take(60).collect(), x["state"].as_str().unwrap_or("").to_string()))
            .collect();
        Project { root: root.into(), s, alive, tasks }
    }

    fn of(&self, want: &[&str]) -> Vec<&(u64, String, String)> {
        self.tasks.iter().filter(|(_, _, st)| want.contains(&st.as_str())).collect()
    }

    fn counts(&self) -> String {
        if self.alive {
            format!("In work {} · Waiting for answer {} · Ready {}", self.of(&["doing", "review"]).len(), self.of(&["ask"]).len(), self.of(&["ready"]).len())
        } else {
            "service not running".into()
        }
    }

    fn name(&self) -> String {
        self.s["project"].as_str().map(String::from).unwrap_or_else(|| crate::project_name(&self.root))
    }
}

fn refresh(t: &TrayIcon, install: &Path, shown: &mut String) {
    let ps: Vec<Project> = fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(Project::read).collect();
    let sig: String = ps.iter().map(|p| format!("{}|{}|{}|{}|{:?}|{}\n", p.root.display(), p.counts(), p.s["focus"], p.s["error"], p.tasks, p.s["version"])).collect();
    if *shown == sig {
        return;
    }
    *shown = sig;
    let (asks, work) = ps.iter().fold((0, 0), |(a, w), p| (a + p.of(&["ask"]).len(), w + p.of(&["doing", "review"]).len()));
    let color = if ps.iter().any(|p| !p.alive || !p.s["error"].is_null()) {
        [214, 48, 49]
    } else if asks > 0 {
        [230, 126, 34]
    } else if work > 0 {
        [52, 120, 246]
    } else {
        [128, 128, 128]
    };
    let _ = t.set_icon(Some(icon(color)));
    let _ = t.set_tooltip(Some(format!("taskq: in work {work} · waiting for answer {asks}")));
    #[cfg(target_os = "macos")]
    t.set_title(Some(if asks == 0 { String::new() } else { asks.to_string() }));

    let item = |id: &str, text: &str, on: bool| MenuItem::with_id(id, text, on, None);
    let m = Menu::new();
    let host = ps.iter().find_map(|p| p.s["host"].as_str()).unwrap_or("?");
    let _ = m.append(&item("head", &format!("taskq {} · {host}", crate::platform::VERSION), false));
    if ps.is_empty() {
        let _ = m.append(&item("none", "No projects: run `taskq service install` in one", false));
    }
    for p in &ps {
        let (asks, work) = (p.of(&["ask"]), p.of(&["doing", "review"]));
        let root = p.root.display().to_string();
        let sub = Submenu::new(format!("{} · {}", p.name(), p.counts()), true);
        let s = &p.s;
        if let Some(e) = s["error"].as_str() {
            let _ = sub.append(&item("error", &format!("error: {}", e.chars().take(80).collect::<String>()), false));
        }
        let issues = s["issues"].as_str().unwrap_or("");
        let _ = sub.append(&item(&format!("open {issues}"), "Board", !issues.is_empty()));
        for (title, list) in [("Waiting for answer", &asks), ("In work", &work)] {
            if list.is_empty() {
                continue;
            }
            let l = Submenu::new(format!("{title} ({})", list.len()), true);
            for (n, name, _) in list.iter() {
                let _ = l.append(&item(&format!("open {issues}/{n}"), &format!("#{n} {name}"), true));
            }
            let _ = sub.append(&l);
        }
        let _ = sub.append(&PredefinedMenuItem::separator());
        let paused = s["focus"].as_array().is_none_or(|f| f.is_empty());
        let saved = fs::read_to_string(crate::dot(&p.root).join("focus.paused")).is_ok_and(|f| !matches!(f.trim(), "" | "off"));
        let _ = sub.append(&match (paused, saved) {
            (false, _) => item(&format!("pause {root}"), "Pause: finish running tasks, take no new", p.alive),
            (true, true) => item(&format!("resume {root}"), "Resume: take new tasks", p.alive),
            // no focus to restore: never `all` by default (§ 4)
            (true, false) => item("resume", "Resume: choose labels with `taskq service focus`", false),
        });
        if let Some(log) = s["log"].as_str() {
            let _ = sub.append(&item(&format!("open {log}"), "Open log", true));
        }
        let _ = sub.append(&item(&format!("open {root}"), "Open project folder", true));
        let _ = sub.append(&PredefinedMenuItem::separator());
        let _ = sub.append(&item(&format!("restart {root}"), "Restart service", p.alive));
        let _ = sub.append(&item(&format!("stop {root}"), "Quit until next logon", true));
        let un = Submenu::new("Uninstall", true);
        let _ = un.append(&item(&format!("uninstall {root}"), "Confirm: remove the service", true));
        let _ = sub.append(&un);
        let _ = m.append(&sub);
    }
    t.set_menu(Some(Box::new(m)));
}

/// A menu id is `<action> <project root>` or `open <path or url>`.
fn act(id: &str) {
    let Some((action, arg)) = id.split_once(' ') else { return };
    let root = Path::new(arg);
    let d = crate::dot(root);
    match action {
        "pause" => crate::pause(root),
        "resume" => {
            let _ = crate::resume(root);
        }
        "restart" => {
            let _ = fs::write(d.join("restart"), "");
            crate::poke(root);
        }
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
        "open" => {
            let opener = if cfg!(windows) { "explorer" } else { "open" };
            let _ = crate::platform::quiet(std::process::Command::new(opener).arg(arg)).spawn();
        }
        _ => {}
    }
}

/// A filled circle in the state's color.
fn icon(rgb: [u8; 3]) -> Icon {
    let n = 32u32;
    let mut px = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            px.extend([
                rgb[0],
                rgb[1],
                rgb[2],
                if dx * dx + dy * dy <= 144.0 { 255 } else { 0 },
            ]);
        }
    }
    Icon::from_rgba(px, n, n).expect("32x32 rgba")
}
