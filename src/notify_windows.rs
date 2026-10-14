//! Windows adapter for M14. The shared core owns modes, cursors, bursts and card payloads.
use super::Post;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::core::{HSTRING, Interface, Result, w};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, IPersistFile, STGM_READ, STGM_READWRITE};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SetCurrentProcessExplicitAppUserModelID};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSGetPropertyKeyFromName};

const APP: &str = "org.taskq.tray";
static NOTIFIER: OnceLock<std::result::Result<ToastNotifier, String>> = OnceLock::new();
static CLICK: OnceLock<Mutex<Box<dyn Fn(String) + Send>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static POSTS: OnceLock<Mutex<HashMap<String, (ToastNotification, u64)>>> = OnceLock::new();

// Preserve an unrelated shortcut rather than taking its name or ownership.
fn shortcut(path: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    unsafe {
        let mut key = windows::Win32::Foundation::PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &mut key)?;
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        let props: IPropertyStore = link.cast()?;
        let name = HSTRING::from(path.as_os_str());
        if path.exists() {
            file.Load(&name, STGM_READWRITE)?;
            let id = props.GetValue(&key)?;
            if windows::core::BSTR::try_from(&id)?.to_string() != APP {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x800700B7u32 as i32)));
            }
        }
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetArguments(w!("tray"))?;
        props.SetValue(&key, &PROPVARIANT::from(APP))?;
        props.Commit()?;
        file.Save(&name, true)?;
        // Read back the saved identity before allowing any toast.
        file.Load(&name, STGM_READ)?;
        if windows::core::BSTR::try_from(&props.GetValue(&key)?)?.to_string() != APP {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005u32 as i32)));
        }
        Ok(())
    }
}

fn setting(s: Result<NotificationSetting>) -> &'static str {
    match s {
        Ok(NotificationSetting::Enabled) => "allowed",
        Ok(NotificationSetting::DisabledForApplication | NotificationSetting::DisabledForUser |
            NotificationSetting::DisabledByGroupPolicy | NotificationSetting::DisabledByManifest) => "blocked",
        _ => "unknown",
    }
}

pub fn status() -> &'static str {
    match NOTIFIER.get() {
        Some(Ok(n)) => setting(n.Setting()),
        _ => "unknown",
    }
}

// Windows has no notification consent prompt for unpackaged desktop apps.
pub fn request() {}

/// Windows has no permission prompt, so no request can fail.
pub fn failure() -> Option<String> {
    None
}

fn xml(p: &Post) -> Result<XmlDocument> {
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(format!("<toast><visual><binding template=\"ToastGeneric\"><text/><text/></binding></visual><audio {} /></toast>",
        if p.sound { "src=\"ms-winsoundevent:Notification.Default\"" } else { "silent=\"true\"" })))?;
    let texts = doc.GetElementsByTagName(&HSTRING::from("text"))?;
    for (i, text) in [&p.title, &p.body].into_iter().enumerate() {
        let node = doc.CreateTextNode(&HSTRING::from(text))?;
        texts.Item(i as u32)?.AppendChild(&node)?;
    }
    Ok(doc)
}

fn tag(id: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, id.as_bytes()).as_ref()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn activation(id: String) -> TypedEventHandler<ToastNotification, windows::core::IInspectable> {
    TypedEventHandler::new(move |_, _| {
        if let Some(f) = CLICK.get().and_then(|f| f.lock().ok()) { f(id.clone()); }
        Ok(())
    })
}

fn send(n: &ToastNotifier, p: &Post) -> Result<bool> {
    if setting(n.Setting()) != "allowed" { return Ok(false) }
    let toast = ToastNotification::CreateToastNotification(&xml(p)?)?;
    let key = tag(&p.id);
    toast.SetTag(&HSTRING::from(&key))?;
    toast.SetGroup(&HSTRING::from(tag(&p.root)))?;
    toast.Activated(&activation(p.id.clone()))?;
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let posts = POSTS.get_or_init(|| Mutex::new(HashMap::new()));
    // Keep the event source alive until dismissed/failed; a summary replaces its previous source.
    for failed in [false, true] {
        let key = key.clone();
        let remove = move || { if let Ok(mut ps) = posts.lock() {
            if ps.get(&key).is_some_and(|p| p.1 == generation) { ps.remove(&key); }
        } };
        if failed {
            toast.Failed(&TypedEventHandler::new(move |_, _| { remove(); Ok(()) }))?;
        } else {
            toast.Dismissed(&TypedEventHandler::new(move |_, _| { remove(); Ok(()) }))?;
        }
    }
    if let Ok(mut ps) = posts.lock() { ps.insert(key.clone(), (toast.clone(), generation)); }
    if let Err(e) = n.Show(&toast) {
        if let Ok(mut ps) = posts.lock() { ps.remove(&key); }
        return Err(e);
    }
    Ok(true)
}

pub fn post(p: &Post) -> bool {
    let Some(Ok(n)) = NOTIFIER.get() else { return false };
    if status() != "allowed" { return false }
    match send(n, p) {
        Ok(posted) => posted,
        Err(e) => { eprintln!("taskq notification: {e}"); false }
    }
}

pub fn watch(f: impl Fn(String) + Send + 'static) {
    let _ = CLICK.set(Mutex::new(Box::new(f)));
    let n = NOTIFIER.get_or_init(|| {
        let init = || -> std::result::Result<ToastNotifier, String> {
            // The tray event loop owns the STA; its lifetime also owns WinRT initialization.
            unsafe { RoInitialize(RO_INIT_SINGLETHREADED).map_err(|e| e.to_string())?; }
            let appdata = std::env::var_os("APPDATA").ok_or("APPDATA unavailable")?;
            let path = std::path::PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs/TaskQ.lnk");
            let exe = crate::platform::default_install().join("taskq-tray.exe");
            if !exe.is_file() { return Err("stable tray executable unavailable".into()) }
            shortcut(&path, &exe).map_err(|e| e.to_string())?;
            unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP)).map_err(|e| e.to_string())?; }
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP)).map_err(|e| e.to_string())
        };
        init()
    });
    if let Err(e) = n { eprintln!("taskq notifications unknown: {e}"); }
}

pub fn open_settings() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", "ms-settings:notifications"])
        .creation_flags(0x08000000).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: M14 Windows permission. Risk: unknown or denied permission becomes healthy and posts.
    #[test]
    fn permission_denial_and_errors() {
        assert_eq!(setting(Ok(NotificationSetting::Enabled)), "allowed");
        for s in [NotificationSetting::DisabledForApplication, NotificationSetting::DisabledForUser,
            NotificationSetting::DisabledByGroupPolicy, NotificationSetting::DisabledByManifest] {
            assert_eq!(setting(Ok(s)), "blocked");
        }
        assert_eq!(setting(Ok(NotificationSetting(99))), "unknown");
        assert_eq!(setting(Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005u32 as i32)))), "unknown");
    }

    // Chain: M14 Windows identity. Risk: registration overwrites another app or saves the wrong identity/target.
    #[test]
    fn shortcut_identity_readback_and_ownership() -> Result<()> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)?; }
        let dir = std::env::temp_dir().join(format!("taskq-toast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("TaskQ.lnk");
        let exe = std::env::current_exe().unwrap();
        shortcut(&path, &exe)?;
        shortcut(&path, &exe)?;
        unsafe {
            let mut key = windows::Win32::Foundation::PROPERTYKEY::default();
            PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &mut key)?;
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(&HSTRING::from(path.as_os_str()), STGM_READWRITE)?;
            let mut args = [0u16; 32];
            link.GetArguments(&mut args)?;
            assert_eq!(String::from_utf16_lossy(&args[..4]), "tray");
            let props: IPropertyStore = link.cast()?;
            props.SetValue(&key, &PROPVARIANT::from("another.app"))?;
            props.Commit()?;
            file.Save(&HSTRING::from(path.as_os_str()), true)?;
        }
        let before = std::fs::read(&path).unwrap();
        assert!(shortcut(&path, &exe).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
        unsafe { windows::Win32::System::WinRT::RoUninitialize(); }
        Ok(())
    }

    // Chain: M14 Windows toast payload. Risk: owner text changes XML/sound or summary identifiers do not replace.
    #[test]
    fn native_xml_preserves_text_sound_and_summary_identity() -> Result<()> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)?; }
        let mut p = Post { root: "C:/project".into(), n: Some(7), title: "<question> & \"owner\"".into(),
            body: "#7 user's task 🦀".into(), sound: false, id: "{\"root\":\"C:/project\",\"sum\":1}".into() };
        for sound in [false, true] {
            p.sound = sound;
            let doc = xml(&p)?;
            let texts = doc.GetElementsByTagName(&HSTRING::from("text"))?;
            assert_eq!(texts.Item(0)?.InnerText()?.to_string(), p.title);
            assert_eq!(texts.Item(1)?.InnerText()?.to_string(), p.body);
            let t = ToastNotification::CreateToastNotification(&doc)?;
            t.SetTag(&HSTRING::from(tag(&p.id)))?;
            t.SetGroup(&HSTRING::from(tag(&p.root)))?;
            assert_eq!(t.Tag()?.to_string(), tag(&p.id));
            assert_eq!(t.Group()?.to_string(), tag(&p.root));
            assert_eq!(doc.GetXml()?.to_string().contains("silent=\"true\""), !sound);
        }
        // Invoke the production COM event delegate: API evidence, not a native user click.
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(CLICK.set(Mutex::new(Box::new(move |id| { tx.send(id).unwrap(); }))).is_ok());
        activation(p.id.clone()).Invoke(None, None)?;
        assert_eq!(rx.recv().unwrap(), p.id);
        assert_eq!(tag(&p.id).len(), 16);
        assert_ne!(tag(&p.id), tag("another event"));
        unsafe { windows::Win32::System::WinRT::RoUninitialize(); }
        Ok(())
    }
}
