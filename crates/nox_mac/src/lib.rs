//! `nox_mac` — the thin `AppKit` layer (SPEC §13): native context menus, accessibility flags, Dock
//! bounce, beep, the attention chime, system notifications, the `NSWindow` background color,
//! keep-awake power assertions and which Option key is down.
//!
//! Everything here is main-thread `AppKit` messaging through `objc2`; functions are no-ops (or
//! return defaults) where the platform refuses (e.g. notifications from an unbundled binary).

// `cargo lint` runs clippy::pedantic. These lints are allowed on purpose: numeric casts are
// bounded grid / pixel / color math, `# Errors` / `# Panics` sections would only restate the
// signature, hex color literals read better without separators, and long UI builders and
// flag-heavy config structs are clearer kept whole; `assert!(x.is_empty())` reads fine in tests
// and `(n..).find(..)` searches are meant to be unbounded.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::assert_is_empty,
    clippy::maybe_infinite_iter
)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSDictionary, NSString};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSBeep();
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: *const c_void,
        level: u32,
        name: *const c_void,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

// libdispatch (part of libSystem).
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

fn class(name: &str) -> Option<&'static AnyClass> {
    let c = std::ffi::CString::new(name).ok()?;
    AnyClass::get(&c)
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Run `f` on the main queue on a later run-loop turn (outside of any GPUI callback).
pub fn dispatch_main(f: Box<dyn FnOnce() + Send>) {
    extern "C" fn trampoline(ctx: *mut c_void) {
        let f: Box<Box<dyn FnOnce() + Send>> = unsafe { Box::from_raw(ctx.cast()) };
        f();
    }
    let ctx = Box::into_raw(Box::new(f)).cast::<c_void>();
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, ctx, trampoline) };
}

// ---- accessibility --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Accessibility {
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
}

/// Read `NSWorkspace.accessibilityDisplayShould…`.
pub fn accessibility() -> Accessibility {
    let Some(cls) = class("NSWorkspace") else { return Accessibility::default() };
    unsafe {
        let ws: *mut AnyObject = msg_send![cls, sharedWorkspace];
        if ws.is_null() {
            return Accessibility::default();
        }
        let reduce_motion: Bool = msg_send![ws, accessibilityDisplayShouldReduceMotion];
        let reduce_transparency: Bool = msg_send![ws, accessibilityDisplayShouldReduceTransparency];
        let increase_contrast: Bool = msg_send![ws, accessibilityDisplayShouldIncreaseContrast];
        Accessibility {
            reduce_motion: reduce_motion.as_bool(),
            reduce_transparency: reduce_transparency.as_bool(),
            increase_contrast: increase_contrast.as_bool(),
        }
    }
}

/// Call `f` whenever the accessibility display options change
/// (`NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`). Call once at startup.
pub fn observe_accessibility(f: impl Fn(Accessibility) + 'static) {
    let Some(cls) = class("NSWorkspace") else { return };
    unsafe {
        let ws: *mut AnyObject = msg_send![cls, sharedWorkspace];
        if ws.is_null() {
            return;
        }
        let center: *mut AnyObject = msg_send![ws, notificationCenter];
        let name = ns("NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification");
        let block = RcBlock::new(move |_note: *mut AnyObject| f(accessibility()));
        let queue_cls = class("NSOperationQueue").unwrap();
        let queue: *mut AnyObject = msg_send![queue_cls, mainQueue];
        let _token: *mut AnyObject = msg_send![
            center,
            addObserverForName: &*name,
            object: std::ptr::null_mut::<AnyObject>(),
            queue: queue,
            usingBlock: &*block
        ];
        // The observer lives for the rest of the process.
        std::mem::forget(block);
    }
}

// ---- small app-level calls -----------------------------------------------------------------

pub fn beep() {
    unsafe { NSBeep() };
}

/// Bounce the Dock icon once (`NSInformationalRequest`).
pub fn request_attention() {
    let Some(cls) = class("NSApplication") else { return };
    unsafe {
        let app: *mut AnyObject = msg_send![cls, sharedApplication];
        let _: isize = msg_send![app, requestUserAttention: 10isize];
    }
}

/// Whether the app runs from an `.app` bundle with an identifier (needed for notifications).
pub fn bundle_identifier() -> Option<String> {
    let cls = class("NSBundle")?;
    unsafe {
        let bundle: *mut AnyObject = msg_send![cls, mainBundle];
        if bundle.is_null() {
            return None;
        }
        let id: *mut NSString = msg_send![bundle, bundleIdentifier];
        if id.is_null() {
            return None;
        }
        Some((*id).to_string())
    }
}

/// `Contents/Resources` of the app bundle, if running from one.
pub fn bundle_resources_dir() -> Option<std::path::PathBuf> {
    let cls = class("NSBundle")?;
    unsafe {
        let bundle: *mut AnyObject = msg_send![cls, mainBundle];
        let path: *mut NSString = msg_send![bundle, resourcePath];
        if path.is_null() {
            return None;
        }
        Some(std::path::PathBuf::from((*path).to_string()))
    }
}

/// Which Option keys are currently held: `(left, right)`, from the device-dependent bits of
/// `[NSEvent modifierFlags]` (`NX_DEVICELALTKEYMASK` / `NX_DEVICERALTKEYMASK`).
pub fn option_keys_down() -> (bool, bool) {
    let Some(cls) = class("NSEvent") else { return (false, false) };
    let flags: usize = unsafe { msg_send![cls, modifierFlags] };
    (flags & 0x20 != 0, flags & 0x40 != 0)
}

/// Set the background color of the window that hosts `ns_view` (so areas exposed during live
/// resize never flash white).
pub unsafe fn set_window_background(ns_view: *mut c_void, r: f64, g: f64, b: f64, a: f64) {
    if ns_view.is_null() {
        return;
    }
    let Some(color_cls) = class("NSColor") else { return };
    unsafe {
        let view = ns_view.cast::<AnyObject>();
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return;
        }
        let color: *mut AnyObject = msg_send![color_cls, colorWithSRGBRed: r, green: g, blue: b, alpha: a];
        let _: () = msg_send![window, setBackgroundColor: color];
    }
}

// ---- attention chime -------------------------------------------------------------------------

struct SoundPtr(*mut AnyObject);
unsafe impl Send for SoundPtr {}

static SOUND: Mutex<Option<(String, SoundPtr)>> = Mutex::new(None);

/// Play a sound file through `NSSound` at `volume` (0.0–1.0). The `NSSound` is cached per path.
pub fn play_sound(path: &std::path::Path, volume: f32) {
    let Some(cls) = class("NSSound") else { return };
    let key = path.to_string_lossy().into_owned();
    let mut guard = SOUND.lock().unwrap();
    unsafe {
        let sound = match guard.as_ref() {
            Some((p, s)) if *p == key => s.0,
            _ => {
                let alloc: *mut AnyObject = msg_send![cls, alloc];
                let s: *mut AnyObject =
                    msg_send![alloc, initWithContentsOfFile: &*ns(&key), byReference: true];
                if s.is_null() {
                    return;
                }
                if let Some((_, old)) = guard.take() {
                    let _: () = msg_send![old.0, release];
                }
                *guard = Some((key, SoundPtr(s)));
                s
            }
        };
        let _: Bool = msg_send![sound, stop];
        let _: () = msg_send![sound, setVolume: volume.clamp(0.0, 1.0)];
        let _: Bool = msg_send![sound, play];
    }
}

// ---- keep awake ------------------------------------------------------------------------------

static ASSERTION: Mutex<Option<u32>> = Mutex::new(None);

/// Hold or release a `PreventUserIdleSystemSleep` assertion (the display may still sleep).
pub fn set_keep_awake(on: bool, reason: &str) {
    let mut guard = ASSERTION.lock().unwrap();
    match (on, *guard) {
        (true, None) => {
            let ty = ns("PreventUserIdleSystemSleep");
            let name = ns(reason);
            let mut id = 0u32;
            let rc = unsafe {
                IOPMAssertionCreateWithName(
                    Retained::as_ptr(&ty).cast(),
                    255,
                    Retained::as_ptr(&name).cast(),
                    &raw mut id,
                )
            };
            if rc == 0 {
                *guard = Some(id);
            }
        }
        (false, Some(id)) => {
            unsafe { IOPMAssertionRelease(id) };
            *guard = None;
        }
        _ => {}
    }
}

pub fn keep_awake_active() -> bool {
    ASSERTION.lock().unwrap().is_some()
}

// ---- native context menus -------------------------------------------------------------------

/// One item of a native menu. `tag < 0` with an empty title is a separator.
#[derive(Debug, Clone)]
pub struct MenuItem {
    pub title: String,
    pub tag: i64,
    pub enabled: bool,
    pub checked: bool,
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(title: impl Into<String>, tag: i64) -> Self {
        Self { title: title.into(), tag, enabled: true, checked: false, submenu: vec![] }
    }
    #[must_use]
    pub fn separator() -> Self {
        Self { title: String::new(), tag: -1, enabled: false, checked: false, submenu: vec![] }
    }
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
    #[must_use]
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn submenu(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self { title: title.into(), tag: -1, enabled: true, checked: false, submenu: items }
    }
}

static MENU_CHOICE: AtomicI64 = AtomicI64::new(-1);

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NoxMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(itemClicked:))]
        fn item_clicked(&self, sender: *mut AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            MENU_CHOICE.store(tag as i64, Ordering::SeqCst);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

unsafe fn build_menu(items: &[MenuItem], target: &MenuTarget) -> *mut AnyObject {
    let menu_cls = class("NSMenu").unwrap();
    let item_cls = class("NSMenuItem").unwrap();
    unsafe {
        let alloc: *mut AnyObject = msg_send![menu_cls, alloc];
        let menu: *mut AnyObject = msg_send![alloc, initWithTitle: &*ns("")];
        let _: () = msg_send![menu, setAutoenablesItems: false];
        for it in items {
            if it.title.is_empty() && it.submenu.is_empty() {
                let sep: *mut AnyObject = msg_send![item_cls, separatorItem];
                let _: () = msg_send![menu, addItem: sep];
                continue;
            }
            let alloc: *mut AnyObject = msg_send![item_cls, alloc];
            let action: Option<Sel> = if it.submenu.is_empty() { Some(sel!(itemClicked:)) } else { None };
            let item: *mut AnyObject = msg_send![
                alloc,
                initWithTitle: &*ns(&it.title),
                action: action,
                keyEquivalent: &*ns("")
            ];
            let _: () = msg_send![item, setTag: it.tag as isize];
            let _: () = msg_send![item, setEnabled: it.enabled];
            if it.checked {
                let _: () = msg_send![item, setState: 1isize];
            }
            if it.submenu.is_empty() {
                let _: () = msg_send![item, setTarget: target];
            } else {
                let sub = build_menu(&it.submenu, target);
                let _: () = msg_send![sub, setTitle: &*ns(&it.title)];
                let _: () = msg_send![item, setSubmenu: sub];
                let _: () = msg_send![sub, release];
            }
            let _: () = msg_send![menu, addItem: item];
            let _: () = msg_send![item, release];
        }
        menu
    }
}

/// Show a native context menu at the mouse location. The chosen item's tag (or `None` when
/// dismissed) is passed to `on_choice`. The menu is shown on a later run-loop turn, outside
/// any GPUI callback, so the menu's tracking loop can't re-enter GPUI.
pub fn show_context_menu(items: Vec<MenuItem>, on_choice: Box<dyn FnOnce(Option<i64>) + Send>) {
    dispatch_main(Box::new(move || {
        let Some(mtm) = MainThreadMarker::new() else { return on_choice(None) };
        let target = MenuTarget::new(mtm);
        MENU_CHOICE.store(-1, Ordering::SeqCst);
        unsafe {
            let menu = build_menu(&items, &target);
            let event_cls = class("NSEvent").unwrap();
            let loc: objc2_foundation::NSPoint = msg_send![event_cls, mouseLocation];
            let _: Bool = msg_send![
                menu,
                popUpMenuPositioningItem: std::ptr::null_mut::<AnyObject>(),
                atLocation: loc,
                inView: std::ptr::null_mut::<AnyObject>()
            ];
            let _: () = msg_send![menu, release];
        }
        let tag = MENU_CHOICE.swap(-1, Ordering::SeqCst);
        on_choice((tag >= 0).then_some(tag));
    }));
}

// ---- system notifications -------------------------------------------------------------------

/// Authorization state as last observed: 0 unknown, 1 denied, 2 authorized.
static AUTH: AtomicU8 = AtomicU8::new(0);
static AUTH_REQUESTED: AtomicBool = AtomicBool::new(false);
type ClickHandler = Box<dyn Fn(String) + Send + Sync>;
static CLICK_HANDLER: OnceLock<ClickHandler> = OnceLock::new();
static DELEGATE: Mutex<Option<DelegatePtr>> = Mutex::new(None);

struct DelegatePtr(#[allow(dead_code)] Retained<NotificationDelegate>);
unsafe impl Send for DelegatePtr {}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "NoxNotificationDelegate"]
    struct NotificationDelegate;

    impl NotificationDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: *mut AnyObject,
            response: *mut AnyObject,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            unsafe {
                let note: *mut AnyObject = msg_send![response, notification];
                let request: *mut AnyObject = msg_send![note, request];
                let id: *mut NSString = msg_send![request, identifier];
                if !id.is_null()
                    && let Some(cb) = CLICK_HANDLER.get()
                {
                    cb((*id).to_string());
                }
            }
            handler.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: *mut AnyObject,
            _note: *mut AnyObject,
            handler: &block2::DynBlock<dyn Fn(usize)>,
        ) {
            // Banner + list, no sound (nox plays its own chime).
            handler.call(((1 << 4) | (1 << 3),));
        }
    }
);

fn notification_center() -> Option<*mut AnyObject> {
    // UNUserNotificationCenter raises for processes without a bundle identifier.
    bundle_identifier()?;
    let cls = class("UNUserNotificationCenter")?;
    let center: *mut AnyObject = unsafe { msg_send![cls, currentNotificationCenter] };
    (!center.is_null()).then_some(center)
}

/// Install the click handler (receives the notification identifier, see [`notify`]) and the
/// center delegate. Call once at startup; cheap, requests no permission.
pub fn init_notifications(on_click: impl Fn(String) + Send + Sync + 'static) {
    let _ = CLICK_HANDLER.set(Box::new(on_click));
    let Some(center) = notification_center() else { return };
    let delegate: Retained<NotificationDelegate> = unsafe { msg_send![NotificationDelegate::alloc(), init] };
    unsafe {
        let _: () = msg_send![center, setDelegate: &*delegate];
    }
    *DELEGATE.lock().unwrap() = Some(DelegatePtr(delegate));
    refresh_authorization();
}

/// Re-read the authorization status (async; see [`notifications_denied`]).
pub fn refresh_authorization() {
    let Some(center) = notification_center() else { return };
    let block = RcBlock::new(|settings: *mut AnyObject| {
        let status: isize = unsafe { msg_send![settings, authorizationStatus] };
        // UNAuthorizationStatus: 0 notDetermined, 1 denied, 2 authorized, 3 provisional, 4 ephemeral
        AUTH.store(
            match status {
                1 => 1,
                2..=4 => 2,
                _ => 0,
            },
            Ordering::SeqCst,
        );
    });
    unsafe {
        let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*block];
    }
}

/// Whether the user denied notifications for nox (Settings → Terminal shows a hint then).
pub fn notifications_denied() -> bool {
    AUTH.load(Ordering::SeqCst) == 1
}

/// Post (or replace) the notification `id` — one per pane, so a new one replaces the old.
/// Silent at the system level. Permission is requested on the first post.
pub fn notify(id: &str, title: &str, body: &str) {
    let Some(center) = notification_center() else {
        notify_fallback(title, body);
        return;
    };
    if !AUTH_REQUESTED.swap(true, Ordering::SeqCst) && AUTH.load(Ordering::SeqCst) != 2 {
        let (id, title, body) = (id.to_string(), title.to_string(), body.to_string());
        let block = RcBlock::new(move |granted: Bool, _err: *mut AnyObject| {
            AUTH.store(if granted.as_bool() { 2 } else { 1 }, Ordering::SeqCst);
            if granted.as_bool() {
                let (id, title, body) = (id.clone(), title.clone(), body.clone());
                dispatch_main(Box::new(move || post(&id, &title, &body)));
            }
        });
        unsafe {
            // UNAuthorizationOptionAlert (1 << 2); no sound, no badge.
            let _: () = msg_send![
                center,
                requestAuthorizationWithOptions: 1usize << 2,
                completionHandler: &*block
            ];
        }
        return;
    }
    if AUTH.load(Ordering::SeqCst) == 1 {
        return;
    }
    post(id, title, body);
}

fn post(id: &str, title: &str, body: &str) {
    let Some(center) = notification_center() else { return };
    let (Some(content_cls), Some(request_cls)) =
        (class("UNMutableNotificationContent"), class("UNNotificationRequest"))
    else {
        return;
    };
    unsafe {
        let content: *mut AnyObject = msg_send![content_cls, new];
        let _: () = msg_send![content, setTitle: &*ns(title)];
        let _: () = msg_send![content, setBody: &*ns(body)];
        let _: () = msg_send![content, setThreadIdentifier: &*ns(id)];
        let key = ns("id");
        let val = ns(id);
        let info = NSDictionary::from_slices(&[&*key], &[&*val]);
        let _: () = msg_send![content, setUserInfo: &*info];
        let request: *mut AnyObject = msg_send![
            request_cls,
            requestWithIdentifier: &*ns(id),
            content: content,
            trigger: std::ptr::null_mut::<AnyObject>()
        ];
        let _: () = msg_send![
            center,
            addNotificationRequest: request,
            withCompletionHandler: std::ptr::null_mut::<AnyObject>()
        ];
        let _: () = msg_send![content, release];
    }
}

/// Remove a delivered notification (the pane was visited or closed).
pub fn remove_notification(id: &str) {
    let Some(center) = notification_center() else { return };
    let ids = NSArray::from_retained_slice(&[ns(id)]);
    unsafe {
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: &*ids];
    }
}

/// Development fallback (unbundled binary): `osascript display notification`.
fn notify_fallback(title: &str, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\"", esc(body), esc(title));
    let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).spawn();
}

/// Open System Settings → Notifications.
pub fn open_notification_settings() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
        .spawn();
}

/// Whether ⇧ is held right now (`NSEventModifierFlagShift`).
pub fn shift_key_down() -> bool {
    let Some(cls) = class("NSEvent") else { return false };
    let flags: usize = unsafe { msg_send![cls, modifierFlags] };
    flags & (1 << 17) != 0
}

/// Set the Dock / app-switcher icon from image data (PNG, WebP, …). Used when nox runs as a
/// bare binary (e.g. `cargo run`), where there is no bundle icon.
pub fn set_app_icon(data: &[u8]) {
    let (Some(app_cls), Some(img_cls), Some(data_cls)) =
        (class("NSApplication"), class("NSImage"), class("NSData"))
    else {
        return;
    };
    unsafe {
        let nsdata: *mut AnyObject =
            msg_send![data_cls, dataWithBytes: data.as_ptr().cast::<c_void>(), length: data.len()];
        if nsdata.is_null() {
            return;
        }
        let alloc: *mut AnyObject = msg_send![img_cls, alloc];
        let image: *mut AnyObject = msg_send![alloc, initWithData: nsdata];
        if image.is_null() {
            return;
        }
        let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
}
