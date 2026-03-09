//! Scripting — exposes Nexus session state via Apple Events / AppleScript.
//!
//! Cocoa Scripting: the `.sdef` file maps AppleScript syntax to ObjC classes
//! registered at runtime. macOS handles event dispatch; our classes return data
//! from the `SessionRegistry`.
//!
//! ```applescript
//! tell application "Nexus"
//!     get every window
//!     get tty of every session of window 1
//!     set bounds of window 1 to {0, 0, 800, 600}
//!     set index of window 1 to 1
//! end tell
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ============================================================================
// Data types
// ============================================================================

#[derive(Debug, Clone)]
pub struct WindowSnapshot {
    pub id: u64,
    pub name: String,
    pub sessions: Vec<SessionSnapshot>,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub unique_id: String,
    pub tty: String,
    pub name: String,
    pub cwd: String,
    pub columns: u16,
    pub rows: u16,
    pub running_command: String,
    pub is_busy: bool,
    pub profile_name: String,
}

// ============================================================================
// Registry
// ============================================================================

#[derive(Debug, Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<u64, WindowSnapshot>>>,
    /// Maps window_id → raw NSWindow pointer (for scripting lookups).
    nswindow_ptrs: Arc<Mutex<HashMap<u64, usize>>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            nswindow_ptrs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn update_window(&self, snapshot: WindowSnapshot) {
        self.inner.lock().unwrap().insert(snapshot.id, snapshot);
    }

    pub fn remove_window(&self, id: u64) {
        self.inner.lock().unwrap().remove(&id);
        self.nswindow_ptrs.lock().unwrap().remove(&id);
    }

    pub fn set_nswindow_ptr(&self, window_id: u64, ptr: usize) {
        self.nswindow_ptrs.lock().unwrap().insert(window_id, ptr);
    }

    pub fn get_nswindow_ptr(&self, window_id: u64) -> Option<usize> {
        self.nswindow_ptrs.lock().unwrap().get(&window_id).copied()
    }

    pub fn snapshot(&self) -> Vec<WindowSnapshot> {
        let map = self.inner.lock().unwrap();
        let mut windows: Vec<_> = map.values().cloned().collect();
        windows.sort_by_key(|w| w.id);
        windows
    }
}

// ============================================================================
// Global registry (for ObjC scripting classes)
// ============================================================================

static GLOBAL_REGISTRY: std::sync::OnceLock<SessionRegistry> = std::sync::OnceLock::new();

fn global_registry() -> Option<&'static SessionRegistry> {
    GLOBAL_REGISTRY.get()
}

// ============================================================================
// Init
// ============================================================================

static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// Initialize scripting (idempotent). Registers ObjC classes for Cocoa Scripting.
pub fn init(registry: SessionRegistry) {
    INIT_ONCE.call_once(|| {
        let _ = GLOBAL_REGISTRY.set(registry);
        #[cfg(target_os = "macos")]
        apple_events::register_scripting_classes();
    });
}

// ============================================================================
// TTY lookup
// ============================================================================

#[cfg(target_os = "macos")]
pub fn tty_for_pid(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "tty="])
        .output()
        .ok()?;
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tty.is_empty() || tty == "??" { return None; }
    if tty.starts_with('/') { Some(tty) } else { Some(format!("/dev/{tty}")) }
}

#[cfg(not(target_os = "macos"))]
pub fn tty_for_pid(_pid: u32) -> Option<String> {
    None
}

// ============================================================================
// macOS Cocoa Scripting bridge
// ============================================================================

#[cfg(target_os = "macos")]
#[allow(unsafe_op_in_unsafe_fn)]
mod apple_events {
    use super::*;

    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSString, NSPoint, NSRect, NSSize};

    // ========================================================================
    // Class registration
    // ========================================================================

    pub fn register_scripting_classes() {
        register_class_nexus_script_session();
        register_class_nexus_script_window();
        // App delegate patching is deferred — StrataAppDelegate may not exist yet.
        // We retry on a background thread until it's available.
        std::thread::spawn(|| {
            for _ in 0..100 {
                if patch_app_delegate() { return; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
    }

    fn register_class_nexus_script_session() {
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let cls = objc2::ffi::objc_allocateClassPair(
                superclass as *const _ as *mut _, c"NexusScriptSession".as_ptr(), 0,
            );
            if cls.is_null() { return; }

            add_ivar_u64(cls, c"_windowID");
            add_ivar_usize(cls, c"_sessionIndex");

            add_method(cls, sel!(objectSpecifier), session_object_specifier as *const _, c"@@:");
            add_method(cls, sel!(uniqueID), session_unique_id as *const _, c"@@:");
            add_method(cls, sel!(tty), session_tty as *const _, c"@@:");
            add_method(cls, sel!(name), session_name as *const _, c"@@:");
            add_method(cls, sel!(cwd), session_cwd as *const _, c"@@:");
            add_method(cls, sel!(columns), session_columns as *const _, c"@@:");
            add_method(cls, sel!(rows), session_rows as *const _, c"@@:");
            add_method(cls, sel!(runningCommand), session_running_command as *const _, c"@@:");
            add_method(cls, sel!(isBusy), session_is_busy as *const _, c"@@:");
            add_method(cls, sel!(profileName), session_profile_name as *const _, c"@@:");

            objc2::ffi::objc_registerClassPair(cls);
        }
    }

    fn register_class_nexus_script_window() {
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let cls = objc2::ffi::objc_allocateClassPair(
                superclass as *const _ as *mut _, c"NexusScriptWindow".as_ptr(), 0,
            );
            if cls.is_null() { return; }

            add_ivar_u64(cls, c"_windowID");

            add_method(cls, sel!(objectSpecifier), window_object_specifier as *const _, c"@@:");
            add_method(cls, sel!(uniqueID), window_unique_id as *const _, c"@@:");
            add_method(cls, sel!(name), window_name as *const _, c"@@:");
            add_method(cls, sel!(boundsAsRect), window_bounds as *const _, c"@@:");
            add_method(cls, sel!(setBoundsAsRect:), window_set_bounds as *const _, c"v@:@");
            add_method(cls, sel!(orderedIndex), window_ordered_index as *const _, c"@@:");
            add_method(cls, sel!(setOrderedIndex:), window_set_ordered_index as *const _, c"v@:@");
            add_method(cls, sel!(sessions), window_sessions as *const _, c"@@:");

            objc2::ffi::objc_registerClassPair(cls);
        }
    }

    fn patch_app_delegate() -> bool {
        unsafe {
            let Some(cls) = AnyClass::get("StrataAppDelegate") else { return false; };
            let cls = cls as *const _ as *mut objc2::ffi::objc_class;

            add_method(cls, sel!(orderedScriptingWindows),
                app_ordered_scripting_windows as *const _, c"@@:");
            add_method(cls, sel!(application:delegateHandlesKey:),
                app_delegate_handles_key as *const _, c"B@:@@");
            true
        }
    }

    // ========================================================================
    // Raw FFI helpers
    // ========================================================================

    unsafe fn add_ivar_u64(cls: *mut objc2::ffi::objc_class, name: &std::ffi::CStr) {
        objc2::ffi::class_addIvar(
            cls, name.as_ptr(),
            std::mem::size_of::<u64>(), std::mem::align_of::<u64>() as u8,
            c"Q".as_ptr(),
        );
    }

    unsafe fn add_ivar_usize(cls: *mut objc2::ffi::objc_class, name: &std::ffi::CStr) {
        objc2::ffi::class_addIvar(
            cls, name.as_ptr(),
            std::mem::size_of::<usize>(), std::mem::align_of::<usize>() as u8,
            c"Q".as_ptr(),
        );
    }

    unsafe fn add_method(
        cls: *mut objc2::ffi::objc_class, sel: Sel,
        imp: *const std::ffi::c_void, types: &std::ffi::CStr,
    ) {
        objc2::ffi::class_addMethod(
            cls, sel.as_ptr(),
            Some(std::mem::transmute::<*const std::ffi::c_void, unsafe extern "C" fn()>(imp)),
            types.as_ptr(),
        );
    }

    unsafe fn obj_class(obj: *const AnyObject) -> *const objc2::ffi::objc_class {
        objc2::ffi::object_getClass(obj as *const std::ffi::c_void as *const _)
    }

    unsafe fn get_ivar_u64(obj: *const AnyObject, name: &std::ffi::CStr) -> u64 {
        let ivar = objc2::ffi::class_getInstanceVariable(obj_class(obj), name.as_ptr());
        if ivar.is_null() { return 0; }
        *((obj as *const u8).offset(objc2::ffi::ivar_getOffset(ivar)) as *const u64)
    }

    unsafe fn set_ivar_u64(obj: *mut AnyObject, name: &std::ffi::CStr, val: u64) {
        let ivar = objc2::ffi::class_getInstanceVariable(obj_class(obj), name.as_ptr());
        if ivar.is_null() { return; }
        *((obj as *mut u8).offset(objc2::ffi::ivar_getOffset(ivar)) as *mut u64) = val;
    }

    unsafe fn get_ivar_usize(obj: *const AnyObject, name: &std::ffi::CStr) -> usize {
        let ivar = objc2::ffi::class_getInstanceVariable(obj_class(obj), name.as_ptr());
        if ivar.is_null() { return 0; }
        *((obj as *const u8).offset(objc2::ffi::ivar_getOffset(ivar)) as *const usize)
    }

    unsafe fn set_ivar_usize(obj: *mut AnyObject, name: &std::ffi::CStr, val: usize) {
        let ivar = objc2::ffi::class_getInstanceVariable(obj_class(obj), name.as_ptr());
        if ivar.is_null() { return; }
        *((obj as *mut u8).offset(objc2::ffi::ivar_getOffset(ivar)) as *mut usize) = val;
    }

    unsafe fn nsstring(s: &str) -> *mut AnyObject {
        let ns = NSString::from_str(s);
        let ptr = &*ns as *const _ as *mut AnyObject;
        let _: *mut AnyObject = msg_send![ptr, retain];
        ptr
    }

    unsafe fn nsnumber_i64(v: i64) -> *mut AnyObject {
        msg_send![AnyClass::get("NSNumber").unwrap(), numberWithLongLong: v]
    }

    unsafe fn nsnumber_f64(v: f64) -> *mut AnyObject {
        msg_send![AnyClass::get("NSNumber").unwrap(), numberWithDouble: v]
    }

    unsafe fn nsnumber_bool(v: bool) -> *mut AnyObject {
        msg_send![AnyClass::get("NSNumber").unwrap(), numberWithBool: v]
    }

    unsafe fn nsarray(ptrs: &[*mut AnyObject]) -> *mut AnyObject {
        msg_send![AnyClass::get("NSArray").unwrap(), arrayWithObjects: ptrs.as_ptr() count: ptrs.len()]
    }

    fn find_nswindow(window_id: u64) -> Option<*mut AnyObject> {
        let registry = global_registry()?;
        let ptr = registry.get_nswindow_ptr(window_id)?;
        Some(ptr as *mut AnyObject)
    }

    unsafe fn nswindow_z_index(nswin: *mut AnyObject) -> usize {
        let app: *mut AnyObject =
            msg_send![AnyClass::get("NSApplication").unwrap(), sharedApplication];
        let ordered: *mut AnyObject = msg_send![app, orderedWindows];
        let count: usize = msg_send![ordered, count];
        for i in 0..count {
            let w: *mut AnyObject = msg_send![ordered, objectAtIndex: i];
            if std::ptr::eq(w, nswin) { return i + 1; }
        }
        0
    }

    fn get_session(this: *const AnyObject) -> Option<SessionSnapshot> {
        let wid = unsafe { get_ivar_u64(this, c"_windowID") };
        let idx = unsafe { get_ivar_usize(this, c"_sessionIndex") };
        let registry = global_registry()?;
        let snap = registry.snapshot();
        let win = snap.iter().find(|w| w.id == wid)?;
        win.sessions.get(idx).cloned()
    }

    // ========================================================================
    // App delegate
    // ========================================================================

    extern "C" fn app_delegate_handles_key(
        _this: &AnyObject, _sel: Sel, _app: *mut AnyObject, key: *mut AnyObject,
    ) -> Bool {
        unsafe {
            if key.is_null() { return Bool::NO; }
            let s: *const i8 = msg_send![key, UTF8String];
            if s.is_null() { return Bool::NO; }
            let k = std::ffi::CStr::from_ptr(s).to_string_lossy();
            if k == "orderedScriptingWindows" { Bool::YES } else { Bool::NO }
        }
    }

    extern "C" fn app_ordered_scripting_windows(
        _this: &AnyObject, _sel: Sel,
    ) -> *mut AnyObject {
        let Some(registry) = global_registry() else { return unsafe { nsarray(&[]) } };
        let windows = registry.snapshot();
        let cls = AnyClass::get("NexusScriptWindow").unwrap();
        unsafe {
            let objects: Vec<*mut AnyObject> = windows.iter().map(|w| {
                let obj: *mut AnyObject = msg_send![cls, new];
                set_ivar_u64(obj, c"_windowID", w.id);
                obj
            }).collect();
            nsarray(&objects)
        }
    }

    // ========================================================================
    // NexusScriptWindow
    // ========================================================================

    extern "C" fn window_object_specifier(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe {
            let wid = get_ivar_u64(this as *const _, c"_windowID");
            let app: *mut AnyObject =
                msg_send![AnyClass::get("NSApplication").unwrap(), sharedApplication];
            let app_spec: *mut AnyObject = msg_send![app, objectSpecifier];
            let app_desc: *mut AnyObject = msg_send![app, classDescription];
            let key = NSString::from_str("orderedScriptingWindows");
            let uid = nsnumber_i64(wid as i64);
            let alloc: *mut AnyObject =
                msg_send![AnyClass::get("NSUniqueIDSpecifier").unwrap(), alloc];
            msg_send![alloc,
                initWithContainerClassDescription: app_desc,
                containerSpecifier: app_spec,
                key: &*key,
                uniqueID: uid
            ]
        }
    }

    extern "C" fn window_unique_id(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsnumber_i64(get_ivar_u64(this as *const _, c"_windowID") as i64) }
    }

    extern "C" fn window_name(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        let name = global_registry()
            .and_then(|r| r.snapshot().into_iter().find(|w| w.id == wid))
            .map(|w| w.name).unwrap_or_default();
        unsafe { nsstring(&name) }
    }

    extern "C" fn window_bounds(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        unsafe {
            if let Some(nswin) = find_nswindow(wid) {
                let f: NSRect = msg_send![nswin, frame];
                return nsarray(&[
                    nsnumber_f64(f.origin.x), nsnumber_f64(f.origin.y),
                    nsnumber_f64(f.size.width), nsnumber_f64(f.size.height),
                ]);
            }
            nsarray(&[])
        }
    }

    extern "C" fn window_set_bounds(this: &AnyObject, _sel: Sel, value: *mut AnyObject) {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        unsafe {
            if let Some(nswin) = find_nswindow(wid) {
                let count: usize = msg_send![value, count];
                if count >= 4 {
                    let v0: *mut AnyObject = msg_send![value, objectAtIndex: 0usize];
                    let v1: *mut AnyObject = msg_send![value, objectAtIndex: 1usize];
                    let v2: *mut AnyObject = msg_send![value, objectAtIndex: 2usize];
                    let v3: *mut AnyObject = msg_send![value, objectAtIndex: 3usize];
                    let x: f64 = msg_send![v0, doubleValue];
                    let y: f64 = msg_send![v1, doubleValue];
                    let w: f64 = msg_send![v2, doubleValue];
                    let h: f64 = msg_send![v3, doubleValue];
                    let _: () = msg_send![nswin,
                        setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
                        display: true animate: false];
                }
            }
        }
    }

    extern "C" fn window_ordered_index(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        unsafe {
            if let Some(nswin) = find_nswindow(wid) {
                return nsnumber_i64(nswindow_z_index(nswin) as i64);
            }
            nsnumber_i64(0)
        }
    }

    extern "C" fn window_set_ordered_index(this: &AnyObject, _sel: Sel, value: *mut AnyObject) {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        unsafe {
            if let Some(nswin) = find_nswindow(wid) {
                let idx: i64 = msg_send![value, longLongValue];
                if idx <= 1 {
                    let _: () = msg_send![nswin,
                        makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                } else {
                    let _: () = msg_send![nswin,
                        orderFront: std::ptr::null::<AnyObject>()];
                }
            }
        }
    }

    extern "C" fn window_sessions(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        let wid = unsafe { get_ivar_u64(this as *const _, c"_windowID") };
        let Some(registry) = global_registry() else { return unsafe { nsarray(&[]) } };
        let snap = registry.snapshot();
        let Some(win) = snap.iter().find(|w| w.id == wid) else { return unsafe { nsarray(&[]) } };
        let cls = AnyClass::get("NexusScriptSession").unwrap();
        unsafe {
            let objects: Vec<*mut AnyObject> = win.sessions.iter().enumerate().map(|(i, _)| {
                let obj: *mut AnyObject = msg_send![cls, new];
                set_ivar_u64(obj, c"_windowID", wid);
                set_ivar_usize(obj, c"_sessionIndex", i);
                obj
            }).collect();
            nsarray(&objects)
        }
    }

    // ========================================================================
    // NexusScriptSession
    // ========================================================================

    extern "C" fn session_object_specifier(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe {
            let wid = get_ivar_u64(this as *const _, c"_windowID");
            let uid_str = get_session(this as *const _)
                .map(|s| s.unique_id).unwrap_or_default();

            let app: *mut AnyObject =
                msg_send![AnyClass::get("NSApplication").unwrap(), sharedApplication];
            let app_desc: *mut AnyObject = msg_send![app, classDescription];
            let app_spec: *mut AnyObject = msg_send![app, objectSpecifier];
            let spec_cls = AnyClass::get("NSUniqueIDSpecifier").unwrap();

            // Window specifier
            let win_key = NSString::from_str("orderedScriptingWindows");
            let alloc1: *mut AnyObject = msg_send![spec_cls, alloc];
            let win_spec: *mut AnyObject = msg_send![alloc1,
                initWithContainerClassDescription: app_desc,
                containerSpecifier: app_spec,
                key: &*win_key,
                uniqueID: nsnumber_i64(wid as i64)
            ];

            // Session specifier
            let win_desc: *mut AnyObject =
                msg_send![AnyClass::get("NexusScriptWindow").unwrap(), classDescription];
            let sess_key = NSString::from_str("sessions");
            let alloc2: *mut AnyObject = msg_send![spec_cls, alloc];
            msg_send![alloc2,
                initWithContainerClassDescription: win_desc,
                containerSpecifier: win_spec,
                key: &*sess_key,
                uniqueID: nsstring(&uid_str)
            ]
        }
    }

    extern "C" fn session_unique_id(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.unique_id).unwrap_or_default()) }
    }
    extern "C" fn session_tty(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.tty).unwrap_or_default()) }
    }
    extern "C" fn session_name(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.name).unwrap_or_default()) }
    }
    extern "C" fn session_cwd(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.cwd).unwrap_or_default()) }
    }
    extern "C" fn session_columns(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsnumber_i64(get_session(this as *const _).map(|s| s.columns as i64).unwrap_or(0)) }
    }
    extern "C" fn session_rows(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsnumber_i64(get_session(this as *const _).map(|s| s.rows as i64).unwrap_or(0)) }
    }
    extern "C" fn session_running_command(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.running_command).unwrap_or_default()) }
    }
    extern "C" fn session_is_busy(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsnumber_bool(get_session(this as *const _).map(|s| s.is_busy).unwrap_or(false)) }
    }
    extern "C" fn session_profile_name(this: &AnyObject, _sel: Sel) -> *mut AnyObject {
        unsafe { nsstring(&get_session(this as *const _).map(|s| s.profile_name).unwrap_or_default()) }
    }
}
