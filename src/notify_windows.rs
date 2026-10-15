//! Windows adapter for M14. The shared core owns modes, cursors, bursts and card payloads.
use super::Post;
use std::sync::{Mutex, OnceLock};
use windows::core::{BOOL, GUID, HSTRING, IUnknown, Interface, PCWSTR, Ref, Result, implement, w};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::System::Com::{CoCreateInstance, CoRegisterClassObject, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER,
    IClassFactory, IClassFactory_Impl, IPersistFile, REGCLS_MULTIPLEUSE, STGM_READ, STGM_READWRITE};
use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromCLSID, PROPVARIANT, PropVariantToGUID};
use windows::Win32::UI::Notifications::{INotificationActivationCallback, INotificationActivationCallback_Impl, NOTIFICATION_USER_INPUT_DATA};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SetCurrentProcessExplicitAppUserModelID};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSGetPropertyKeyFromName};

const APP: &str = "org.taskq.tray";
static NOTIFIER: OnceLock<std::result::Result<ToastNotifier, String>> = OnceLock::new();
// The shortcut's System.AppUserModel.ToastActivatorCLSID: the OS sends banner and Notification Center clicks to the
// class object the running tray registers under it.
const ACTIVATOR: GUID = GUID::from_u128(0xaf9b407e_49c7_4b89_9b0e_55ea50346e66);
static CLICK: OnceLock<Mutex<Box<dyn Fn(String) + Send>>> = OnceLock::new();

// Preserve an unrelated shortcut rather than taking its name or ownership.
fn shortcut(path: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    unsafe {
        let mut key = windows::Win32::Foundation::PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &mut key)?;
        let mut activator = windows::Win32::Foundation::PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.AppUserModel.ToastActivatorCLSID"), &mut activator)?;
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
        props.SetValue(&activator, &InitPropVariantFromCLSID(&ACTIVATOR)?)?;
        props.Commit()?;
        file.Save(&name, true)?;
        // Read back the saved identity before allowing any toast.
        file.Load(&name, STGM_READ)?;
        if windows::core::BSTR::try_from(&props.GetValue(&key)?)?.to_string() != APP
            || PropVariantToGUID(&props.GetValue(&activator)?)? != ACTIVATOR {
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
        // Windows records an app's setting at its first toast; until then the notifier reports ERROR_NOT_FOUND and
        // the OS default applies.
        Err(e) if e.code() == windows::core::HRESULT(0x80070490u32 as i32) => "allowed",
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
    // the activator receives this as its arguments: the card to open
    doc.DocumentElement()?.SetAttribute(&HSTRING::from("launch"), &HSTRING::from(&p.id))?;
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

#[implement(INotificationActivationCallback)]
struct Activator;

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(&self, _app: &PCWSTR, args: &PCWSTR, _data: *const NOTIFICATION_USER_INPUT_DATA, _count: u32) -> Result<()> {
        let id = if args.is_null() { String::new() } else { unsafe { args.to_string() }.unwrap_or_default() };
        if let Some(f) = CLICK.get().and_then(|f| f.lock().ok()) { f(id); }
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut core::ffi::c_void) -> Result<()> {
        if outer.is_some() { return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into()) }
        let a: INotificationActivationCallback = Activator.into();
        unsafe { a.query(iid, object).ok() }
    }
    fn LockServer(&self, _lock: BOOL) -> Result<()> { Ok(()) }
}

fn send(n: &ToastNotifier, p: &Post) -> Result<bool> {
    if setting(n.Setting()) != "allowed" { return Ok(false) }
    let toast = ToastNotification::CreateToastNotification(&xml(p)?)?;
    let key = tag(&p.id);
    toast.SetTag(&HSTRING::from(&key))?;
    toast.SetGroup(&HSTRING::from(tag(&p.root)))?;
    n.Show(&toast)?;
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
            // Registered for the tray's lifetime on its STA; the event loop pumps the incoming activations.
            let factory: IClassFactory = Factory.into();
            unsafe { CoRegisterClassObject(&ACTIVATOR, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE).map_err(|e| e.to_string())?; }
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

    // Chain: M14 Windows permission. Risk: unknown or denied permission becomes healthy and posts, or a fresh host never posts.
    #[test]
    fn permission_denial_and_errors() {
        assert_eq!(setting(Ok(NotificationSetting::Enabled)), "allowed");
        for s in [NotificationSetting::DisabledForApplication, NotificationSetting::DisabledForUser,
            NotificationSetting::DisabledByGroupPolicy, NotificationSetting::DisabledByManifest] {
            assert_eq!(setting(Ok(s)), "blocked");
        }
        assert_eq!(setting(Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80070490u32 as i32)))), "allowed",
            "no setting recorded before the first toast is the OS default");
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
            let mut activator = windows::Win32::Foundation::PROPERTYKEY::default();
            PSGetPropertyKeyFromName(w!("System.AppUserModel.ToastActivatorCLSID"), &mut activator)?;
            assert_eq!(PropVariantToGUID(&props.GetValue(&activator)?)?, ACTIVATOR);
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
            assert_eq!(doc.DocumentElement()?.GetAttribute(&HSTRING::from("launch"))?.to_string(), p.id);
        }
        // Invoke the production COM activator through its class factory: API evidence, not a native user click.
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(CLICK.set(Mutex::new(Box::new(move |id| { tx.send(id).unwrap(); }))).is_ok());
        let factory: IClassFactory = Factory.into();
        let a: INotificationActivationCallback = unsafe { factory.CreateInstance(None)? };
        unsafe { a.Activate(w!("org.taskq.tray"), &HSTRING::from(&p.id), &[])?; }
        assert_eq!(rx.recv().unwrap(), p.id);
        assert_eq!(tag(&p.id).len(), 16);
        assert_ne!(tag(&p.id), tag("another event"));
        unsafe { windows::Win32::System::WinRT::RoUninitialize(); }
        Ok(())
    }
}
