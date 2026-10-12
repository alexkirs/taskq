//! `taskq tray` (taskq.md § 4): a menu-bar / notification-area icon over `<root>/.taskq/status.json`.
//! No board access and no token: actions go through the same files and commands as the CLI.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub fn run(root: PathBuf) -> Result<(), String> {
    let d = crate::dot(&root);
    if crate::file_age(&d.join("tray")).is_some_and(|a| a < 15) {
        return Err("taskq tray already runs for this project".into());
    }
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::new().build();
    #[cfg(target_os = "macos")]
    {
        // menu bar only: no Dock icon, no app menu
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let mut tray: Option<TrayIcon> = None;
    let mut shown = String::new();
    let mut next = Instant::now();
    event_loop.run(move |event, _, flow| {
        // ponytail: 0.5 s polling for clicks and the status file, an event proxy if latency ever matters
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));
        if let Event::NewEvents(StartCause::Init) = event {
            // macOS needs a running event loop before the icon exists
            tray = TrayIconBuilder::new()
                .with_icon(icon([128, 128, 128]))
                .with_tooltip("taskq")
                .build()
                .ok();
            if tray.is_none() {
                *flow = ControlFlow::Exit;
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            act(&root, &e.id.0);
        }
        if let Some(t) = &tray
            && Instant::now() >= next
        {
            next = Instant::now() + Duration::from_secs(2);
            let _ = fs::write(d.join("tray"), "");
            refresh(t, &d, &mut shown);
        }
    })
}

fn refresh(t: &TrayIcon, d: &Path, shown: &mut String) {
    let s: Value = fs::read_to_string(d.join("status.json"))
        .ok()
        .and_then(|x| serde_json::from_str(&x).ok())
        .unwrap_or(Value::Null);
    let alive = crate::file_age(&d.join("status.json")).is_some_and(|a| a < 300);
    let tasks: Vec<(u64, String, String)> = s["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            (
                x["n"].as_u64().unwrap_or(0),
                x["title"].as_str().unwrap_or("").chars().take(60).collect(),
                x["state"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let of = |want: &[&str]| {
        tasks
            .iter()
            .filter(|(_, _, st)| want.contains(&st.as_str()))
            .collect::<Vec<_>>()
    };
    let (work, asks, ready) = (of(&["doing", "review"]), of(&["ask"]), of(&["ready"]).len());
    let paused = s["focus"].as_array().is_none_or(|f| f.is_empty());
    let counts = if alive {
        format!(
            "In work {} · Waiting for answer {} · Ready {ready}",
            work.len(),
            asks.len()
        )
    } else {
        "service not running".into()
    };
    let sig = format!(
        "{counts}|{paused}|{}|{tasks:?}|{}",
        s["error"], s["version"]
    );
    if *shown == sig {
        return;
    }
    *shown = sig;
    let project = s["project"].as_str().unwrap_or("taskq");
    let color = if !alive || !s["error"].is_null() {
        [214, 48, 49]
    } else if !asks.is_empty() {
        [230, 126, 34]
    } else if !work.is_empty() {
        [52, 120, 246]
    } else {
        [128, 128, 128]
    };
    let _ = t.set_icon(Some(icon(color)));
    let _ = t.set_tooltip(Some(format!("taskq {project}: {counts}")));
    #[cfg(target_os = "macos")]
    t.set_title(Some(if asks.is_empty() {
        String::new()
    } else {
        asks.len().to_string()
    }));

    let issues = s["issues"].as_str().unwrap_or("");
    let item = |id: &str, text: &str, on: bool| MenuItem::with_id(id, text, on, None);
    let m = Menu::new();
    let head = format!(
        "taskq {} · {} · {project}",
        s["version"].as_str().unwrap_or("?"),
        s["host"].as_str().unwrap_or("?")
    );
    let _ = m.append(&item("head", &head, false));
    let _ = m.append(&item("counts", &counts, false));
    if let Some(e) = s["error"].as_str() {
        let _ = m.append(&item(
            "error",
            &format!("error: {}", e.chars().take(80).collect::<String>()),
            false,
        ));
    }
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&item(
        &format!("open {issues}"),
        "Board",
        !issues.is_empty(),
    ));
    for (title, list) in [("Waiting for answer", &asks), ("In work", &work)] {
        if list.is_empty() {
            continue;
        }
        let sub = Submenu::new(format!("{title} ({})", list.len()), true);
        for (n, name, _) in list.iter() {
            let _ = sub.append(&item(
                &format!("open {issues}/{n}"),
                &format!("#{n} {name}"),
                true,
            ));
        }
        let _ = m.append(&sub);
    }
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&if paused {
        item("resume", "Resume: take new tasks", alive)
    } else {
        item("pause", "Pause: finish running tasks, take no new", alive)
    });
    if let Some(log) = s["log"].as_str() {
        let _ = m.append(&item(&format!("open {log}"), "Open log", true));
    }
    if let Some(root) = d.parent() {
        let _ = m.append(&item(
            &format!("open {}", root.display()),
            "Open project folder",
            true,
        ));
    }
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&item("restart", "Restart service", alive));
    let _ = m.append(&item("stop", "Quit until next logon", true));
    let un = Submenu::new("Uninstall", true);
    let _ = un.append(&item("uninstall", "Confirm: remove the service", true));
    let _ = m.append(&un);
    t.set_menu(Some(Box::new(m)));
}

fn act(root: &Path, id: &str) {
    let d = crate::dot(root);
    match id {
        "pause" => {
            let _ = fs::write(
                d.join("focus.paused"),
                fs::read_to_string(d.join("focus")).unwrap_or_default(),
            );
            let _ = fs::write(d.join("focus"), "off");
            crate::poke(root);
        }
        "resume" => {
            let f = fs::read_to_string(d.join("focus.paused"))
                .ok()
                .filter(|f| !matches!(f.trim(), "" | "off"))
                .unwrap_or_else(|| "all".into());
            let _ = fs::write(d.join("focus"), f);
            crate::poke(root);
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
            c.args(["service", id])
                .current_dir(root)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            let _ = crate::platform::quiet(&mut c).spawn();
        }
        _ => {
            if let Some(x) = id.strip_prefix("open ") {
                let opener = if cfg!(windows) { "explorer" } else { "open" };
                let _ = crate::platform::quiet(std::process::Command::new(opener).arg(x)).spawn();
            }
        }
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
