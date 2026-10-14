//! Tray notifications (taskq.md § 4, menu.md M14): new `ask`, `close`, `failed` and `gone` lines of each project's
//! `events` become native OS notifications. The core here is pure over the `notify` value of `<install>/tray.json`
//! (`kinds`: host modes, `projects`: per-project overrides, `cursor`: the last handled event line per project); the
//! macOS adapter posts through UNUserNotificationCenter, which needs the `<install>/TaskQ.app` bundle.

use serde_json::{Value, json};

pub const KINDS: [&str; 4] = ["ask", "close", "failed", "gone"];
/// Off, Silent (menu badge only), Banner, Banner with sound.
pub const MODES: [&str; 4] = ["off", "silent", "banner", "sound"];
/// More than this many banners of one project in one tick become one summary (M14).
const BURST: usize = 3;

pub fn default(kind: &str) -> &'static str {
    match kind {
        "ask" => "sound",
        "failed" | "gone" => "banner",
        _ => "off",
    }
}

fn valid(m: &Value) -> Option<&'static str> {
    MODES.iter().find(|x| m.as_str() == Some(x)).copied()
}

/// The host mode of `kind`: an unknown value reads as the default (the file is kept).
pub fn host(n: &Value, kind: &str) -> &'static str {
    valid(&n["kinds"][kind]).unwrap_or(default(kind))
}

/// The effective mode of `kind` in `root`: its override, else the host mode.
pub fn mode(n: &Value, root: &str, kind: &str) -> &'static str {
    valid(&n["projects"][root][kind]).unwrap_or_else(|| host(n, kind))
}

/// Page action `{kind, value, root?}`: a host mode, or a project override (`inherit` removes it). False: refused.
pub fn set(n: &mut Value, m: &Value) -> bool {
    let Some(kind) = m["kind"].as_str().filter(|k| KINDS.contains(k)) else { return false };
    if !n.is_object() {
        *n = json!({});
    }
    match (m["root"].as_str(), valid(&m["value"])) {
        (Some(root), v) if v.is_some() || m["value"] == "inherit" => {
            if !n["projects"][root].is_object() {
                n["projects"][root] = json!({});
            }
            let p = n["projects"][root].as_object_mut().unwrap();
            match v {
                Some(v) => p.insert(kind.into(), json!(v)),
                None => p.remove(kind),
            };
        }
        (None, Some(v)) => n["kinds"][kind] = json!(v),
        _ => return false,
    }
    true
}

/// True when any kind of any project may post a banner: the first such setting asks the OS for permission.
pub fn wants_banner(n: &Value, roots: &[String]) -> bool {
    KINDS.iter().any(|k| ["banner", "sound"].contains(&host(n, k)) || roots.iter().any(|r| ["banner", "sound"].contains(&mode(n, r, k))))
}

/// One OS notification. `n` is None for a burst summary (a click opens the project).
#[derive(Debug, PartialEq)]
pub struct Post {
    pub root: String,
    pub n: Option<u64>,
    pub title: String,
    pub body: String,
    pub sound: bool,
    /// Unique per event line: the OS replaces a notification with the same identifier.
    pub id: String,
}

pub fn label(kind: &str) -> &'static str {
    match kind {
        "ask" => "Question",
        "close" => "Closed",
        "failed" => "Failed",
        "gone" => "Attempt gone",
        _ => "Event",
    }
}

/// Events of the tray's projects (`root`, `name`, `tasks`, `events` lines `[time, kind, attempt, n]`) after each
/// project's cursor, as posts; the cursor moves past every handled line, Off and Silent included. A project seen for
/// the first time starts at `now`: older events never notify.
pub fn due(n: &mut Value, ps: &[Value], now: i64) -> Vec<Post> {
    if !n.is_object() {
        *n = json!({});
    }
    let mut out = vec![];
    for p in ps {
        let Some(root) = p["root"].as_str() else { continue };
        let lines: Vec<&Value> = p["events"].as_array().into_iter().flatten().collect();
        let line = |e: &Value| format!("{} #{} {} {}", e[0], e[3], e[1].as_str().unwrap_or(""), e[2].as_str().unwrap_or(""));
        let Some(cursor) = n["cursor"][root].as_str().map(String::from) else {
            n["cursor"][root] = json!(now.to_string());
            continue;
        };
        // after the cursor line; when it is gone (trimmed), after its time
        let start = match lines.iter().rposition(|e| line(e) == cursor) {
            Some(i) => i + 1,
            None => {
                let t: i64 = cursor.split(' ').next().and_then(|t| t.parse().ok()).unwrap_or(i64::MAX);
                lines.iter().position(|e| e[0].as_i64().is_some_and(|x| x > t)).unwrap_or(lines.len())
            }
        };
        let new = &lines[start..];
        let Some(last) = new.last() else { continue };
        n["cursor"][root] = json!(line(last));
        let name = p["name"].as_str().unwrap_or(root);
        let shown: Vec<(&Value, &str)> = new.iter().filter_map(|e| {
            let m = mode(n, root, e[1].as_str()?);
            ["banner", "sound"].contains(&m).then_some((*e, m))
        }).collect();
        let sound = shown.iter().any(|(_, m)| *m == "sound");
        // ponytail: a burst is one tick (2 s) of one project, not a sliding 10 s window; one events pass lands in one tick
        if shown.len() > BURST {
            let mut counts: Vec<String> = vec![];
            for k in KINDS {
                let c = shown.iter().filter(|(e, _)| e[1] == k).count();
                if c > 0 {
                    counts.push(format!("{c} {}", label(k).to_lowercase()));
                }
            }
            out.push(Post { root: root.into(), n: None, title: format!("{name} · {} events", shown.len()), body: counts.join(", "), sound, id: json!({"root": root, "e": line(last), "sum": true}).to_string() });
            continue;
        }
        for (e, m) in shown {
            let num = e[3].as_u64();
            let title = p["tasks"].as_array().into_iter().flatten().find(|t| t["n"].as_u64() == num && num.is_some()).and_then(|t| t["title"].as_str());
            let body = match (num, title) {
                (Some(x), Some(t)) => format!("#{x} {t}"),
                (Some(x), None) => format!("#{x}"),
                _ => String::new(),
            };
            out.push(Post { root: root.into(), n: num, title: format!("{name} · {}", label(e[1].as_str().unwrap_or(""))), body, sound: m == "sound", id: json!({"root": root, "n": num, "e": line(e)}).to_string() });
        }
    }
    out
}

/// `Send test`: one sample of `kind` (its host mode decides the sound).
pub fn sample(n: &Value, kind: &str, now: i64) -> Post {
    Post { root: String::new(), n: None, title: format!("taskq · {}", label(kind)), body: "#1 Sample task".into(), sound: host(n, kind) == "sound", id: json!({"test": kind, "at": now}).to_string() }
}

/// OS permission as the Settings page shows it: `allowed`, `blocked`, `not asked`, `unknown` or `unsupported`.
pub use os::{open_settings, post, request, status, watch};

#[cfg(target_os = "macos")]
mod os {
    use super::Post;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::*;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Mutex, OnceLock};

    /// -1 unknown, else UNAuthorizationStatus; refreshed asynchronously.
    static STATUS: AtomicI64 = AtomicI64::new(-1);
    static CLICK: OnceLock<Mutex<Box<dyn Fn(String) + Send>>> = OnceLock::new();
    static DELEGATE: OnceLock<usize> = OnceLock::new();

    /// UNUserNotificationCenter raises outside an app bundle: only `<install>/TaskQ.app` has one.
    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        NSBundle::mainBundle().bundleIdentifier()?;
        Some(UNUserNotificationCenter::currentNotificationCenter())
    }

    fn refresh() {
        let Some(c) = center() else { return };
        let block = RcBlock::new(|s: NonNull<UNNotificationSettings>| {
            STATUS.store(unsafe { s.as_ref() }.authorizationStatus().0 as i64, Ordering::Relaxed);
        });
        c.getNotificationSettingsWithCompletionHandler(&block);
    }

    pub fn status() -> &'static str {
        if center().is_none() {
            return "unsupported";
        }
        refresh();
        match STATUS.load(Ordering::Relaxed) {
            0 => "not asked",
            1 => "blocked",
            2..=4 => "allowed",
            _ => "unknown",
        }
    }

    /// Asks the OS once; a later call while denied changes nothing (the OS asks only once).
    pub fn request() {
        let Some(c) = center() else { return };
        if STATUS.load(Ordering::Relaxed) != 0 {
            return;
        }
        let block = RcBlock::new(|_: objc2::runtime::Bool, _: *mut NSError| refresh());
        c.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &block);
    }

    /// Posts only when allowed: blocked or unknown posts nothing and falls back to no other channel.
    pub fn post(p: &Post) -> bool {
        let Some(c) = center() else { return false };
        if !(2..=4).contains(&STATUS.load(Ordering::Relaxed)) {
            return false;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&p.title));
        content.setBody(&NSString::from_str(&p.body));
        content.setThreadIdentifier(&NSString::from_str(&p.root));
        if p.sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&p.id), &content, None);
        c.addNotificationRequest_withCompletionHandler(&req, None);
        true
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; the delegate has no Drop.
        #[unsafe(super(NSObject))]
        #[name = "TaskqNotifyDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            // the tray is an accessory app: show banners while it is frontmost too
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(&self, _c: &UNUserNotificationCenter, n: &UNNotification, done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
                let sound = n.request().content().sound().is_some();
                let o = UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List;
                done.call((if sound { o | UNNotificationPresentationOptions::Sound } else { o },));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _c: &UNUserNotificationCenter, r: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
                if let Some(f) = CLICK.get().and_then(|f| f.lock().ok()) {
                    f(r.notification().request().identifier().to_string());
                }
                done.call(());
            }
        }
    );

    /// Sets the click handler (it gets the request identifier) and reads the OS status once.
    pub fn watch(f: impl Fn(String) + Send + 'static) {
        let Some(c) = center() else { return };
        if CLICK.set(Mutex::new(Box::new(f))).is_ok() {
            let d: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
            c.setDelegate(Some(ProtocolObject::from_ref(&*d)));
            // the center holds its delegate weakly: keep it for the process
            let _ = DELEGATE.set(Retained::into_raw(d) as usize);
        }
        refresh();
    }

    pub fn open_settings() {
        let _ = std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension").spawn();
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    // ponytail: Windows toasts are not in WS12; the page reads `unsupported`
    pub fn status() -> &'static str {
        "unsupported"
    }
    pub fn request() {}
    pub fn post(_: &super::Post) -> bool {
        false
    }
    pub fn watch(_: impl Fn(String) + Send + 'static) {}
    pub fn open_settings() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj(events: Value) -> Value {
        json!({"root": "/p", "name": "p", "tasks": [{"n": 7, "title": "Fix it"}], "events": events})
    }

    // Chain: M14 modes. Risk: a bad value or a missing override silences or floods a kind.
    #[test]
    fn modes_default_override_and_unknown() {
        let mut n = json!({"kinds": {"close": "loud", "failed": "silent"}});
        assert_eq!([host(&n, "ask"), host(&n, "close"), host(&n, "failed"), host(&n, "gone")], ["sound", "off", "silent", "banner"], "unknown reads as default");
        assert!(set(&mut n, &json!({"kind": "close", "value": "banner", "root": "/p"})));
        assert_eq!((mode(&n, "/p", "close"), mode(&n, "/q", "close")), ("banner", "off"));
        assert!(set(&mut n, &json!({"kind": "close", "value": "inherit", "root": "/p"})));
        assert_eq!(mode(&n, "/p", "close"), "off");
        assert!(!set(&mut n, &json!({"kind": "close", "value": "inherit"})), "host has no inherit");
        assert!(!set(&mut n, &json!({"kind": "blocked", "value": "banner"})));
        assert!(set(&mut n, &json!({"kind": "ask", "value": "off"})));
        assert!(!wants_banner(&json!({"kinds": {"ask": "off", "failed": "silent", "gone": "off"}}), &["/p".into()]));
        assert!(wants_banner(&json!({"kinds": {"ask": "off", "failed": "silent", "gone": "off"}, "projects": {"/p": {"close": "banner"}}}), &["/p".into()]));
    }

    // Chain: M14 once per event across restarts. Risk: a restart repeats or floods old events.
    #[test]
    fn cursor_once_per_event_across_restart() {
        let old = json!([[100, "ask", "5", 7]]);
        let mut n = json!(null);
        assert!(due(&mut n, &[proj(old.clone())], 150).is_empty(), "first start: history never notifies");
        let ev = json!([[100, "ask", "5", 7], [200, "failed", "6", 7], [200, "close", "6", 8]]);
        let p = due(&mut n, &[proj(ev.clone())], 210);
        assert_eq!(p.len(), 1, "close is off by default");
        assert_eq!((p[0].title.as_str(), p[0].body.as_str(), p[0].n, p[0].sound), ("p · Failed", "#7 Fix it", Some(7), false));
        // a restart reloads the saved value: nothing again
        let mut saved: Value = serde_json::from_str(&n.to_string()).unwrap();
        assert!(due(&mut saved, &[proj(ev)], 220).is_empty());
        // the cursor line trimmed away: later times only
        let ev = json!([[200, "close", "6", 8], [300, "ask", "9", 7]]);
        let p = due(&mut saved, &[proj(ev)], 310);
        assert_eq!((p.len(), p[0].sound), (1, true));
        // silent and off still move the cursor
        set(&mut saved, &json!({"kind": "ask", "value": "silent"}));
        assert!(due(&mut saved, &[proj(json!([[300, "ask", "9", 7], [400, "ask", "10", 7]]))], 410).is_empty());
        set(&mut saved, &json!({"kind": "ask", "value": "banner"}));
        assert!(due(&mut saved, &[proj(json!([[300, "ask", "9", 7], [400, "ask", "10", 7]]))], 420).is_empty());
    }

    // Chain: M14 burst. Risk: a busy pass floods the screen.
    #[test]
    fn burst_becomes_one_summary() {
        let mut n = json!({"cursor": {"/p": "0"}});
        let ev = json!([[10, "failed", "1", 1], [10, "failed", "2", 2], [10, "ask", "3", 3], [10, "gone", "4", 4], [10, "close", "5", 5]]);
        let p = due(&mut n, &[proj(ev)], 20);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].title.as_str(), p[0].body.as_str(), p[0].n, p[0].sound), ("p · 4 events", "1 question, 2 failed, 1 attempt gone", None, true));
        let mut n = json!({"cursor": {"/p": "0"}});
        assert_eq!(due(&mut n, &[proj(json!([[10, "failed", "1", 1], [10, "failed", "2", 2], [10, "ask", "3", 3]]))], 20).len(), 3, "three stay single");
    }
}
