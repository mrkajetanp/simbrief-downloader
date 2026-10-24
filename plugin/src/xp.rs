//! Small safe wrappers over the raw XPLM calls. Everything here must run on the sim thread.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;

use crate::ffi;

const LOG_PREFIX: &str = "[SimBrief Downloader] ";

/// C string from arbitrary text; interior NULs are dropped rather than failing.
pub fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// Writes one line to X-Plane's Log.txt.
pub fn log(message: &str) {
    let line = c_string(&format!("{LOG_PREFIX}{message}\n"));
    // SAFETY: `line` is a valid NUL-terminated string that outlives the call.
    unsafe { ffi::XPLMDebugString(line.as_ptr()) }
}

pub fn enable_native_paths() {
    let feature = c_string("XPLM_USE_NATIVE_PATHS");
    // SAFETY: valid C string; unknown features are ignored by X-Plane.
    unsafe { ffi::XPLMEnableFeature(feature.as_ptr(), 1) }
}

/// The X-Plane root directory. Requires native paths to be enabled.
pub fn system_path() -> PathBuf {
    // The SDK asks for at least 512 bytes; use more to be safe with long native paths.
    let mut buffer = vec![0 as c_char; 4096];
    // SAFETY: X-Plane writes a NUL-terminated path into the buffer, which is large enough.
    let path = unsafe {
        ffi::XPLMGetSystemPath(buffer.as_mut_ptr());
        CStr::from_ptr(buffer.as_ptr())
    };
    PathBuf::from(path.to_string_lossy().into_owned())
}

/// Copies a string into one of the 256-byte buffers X-Plane passes to `XPluginStart`.
///
/// # Safety
/// `out` must point to a writable buffer of at least 256 bytes.
pub unsafe fn write_start_string(out: *mut c_char, text: &str) {
    let bytes = text.as_bytes();
    let len = bytes.len().min(255);
    // SAFETY: caller guarantees 256 writable bytes; we write at most 255 plus the NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), out, len);
        *out.add(len) = 0;
    }
}

pub struct DataRef(ffi::XPLMDataRef);

impl DataRef {
    pub fn find(name: &str) -> Option<DataRef> {
        let name = c_string(name);
        // SAFETY: valid C string; returns NULL if the dataref does not exist.
        let handle = unsafe { ffi::XPLMFindDataRef(name.as_ptr()) };
        (!handle.is_null()).then_some(DataRef(handle))
    }

    /// Reads a byte-array dataref, up to `max` bytes.
    pub fn get_bytes(&self, max: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; max];
        let max = c_int::try_from(max).unwrap_or(c_int::MAX);
        // SAFETY: the buffer has room for `max` bytes and X-Plane writes at most that many.
        let read = unsafe { ffi::XPLMGetDatab(self.0, buffer.as_mut_ptr().cast(), 0, max) };
        buffer.truncate(usize::try_from(read).unwrap_or(0));
        buffer
    }
}

/// A submenu under the Plugins menu. Item refs are passed to the handler as integers.
pub struct Menu {
    plugins_menu: ffi::XPLMMenuID,
    plugins_item: c_int,
    menu: ffi::XPLMMenuID,
}

impl Menu {
    pub fn create(
        title: &str,
        items: &[(&str, usize)],
        handler: ffi::XPLMMenuHandler_f,
    ) -> Option<Menu> {
        let title = c_string(title);
        // SAFETY: valid strings and a handler with the correct signature. The returned IDs
        // stay valid until destroyed in `Drop`.
        unsafe {
            let plugins_menu = ffi::XPLMFindPluginsMenu();
            let plugins_item =
                ffi::XPLMAppendMenuItem(plugins_menu, title.as_ptr(), std::ptr::null_mut(), 0);
            if plugins_item < 0 {
                return None;
            }
            let menu = ffi::XPLMCreateMenu(
                title.as_ptr(),
                plugins_menu,
                plugins_item,
                handler,
                std::ptr::null_mut(),
            );
            if menu.is_null() {
                ffi::XPLMRemoveMenuItem(plugins_menu, plugins_item);
                return None;
            }
            for (name, id) in items {
                let name = c_string(name);
                ffi::XPLMAppendMenuItem(menu, name.as_ptr(), *id as *mut c_void, 0);
            }
            Some(Menu {
                plugins_menu,
                plugins_item,
                menu,
            })
        }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: both IDs were created in `create` and are destroyed exactly once.
        unsafe {
            ffi::XPLMDestroyMenu(self.menu);
            ffi::XPLMRemoveMenuItem(self.plugins_menu, self.plugins_item);
        }
    }
}

/// A custom command with a registered handler; unregistered on drop.
pub struct Command {
    command: ffi::XPLMCommandRef,
    handler: ffi::XPLMCommandCallback_f,
    refcon: *mut c_void,
}

impl Command {
    pub fn create(
        name: &str,
        description: &str,
        handler: ffi::XPLMCommandCallback_f,
        id: usize,
    ) -> Option<Command> {
        let name = c_string(name);
        let description = c_string(description);
        let refcon = id as *mut c_void;
        // SAFETY: valid strings; the handler has the correct signature. XPLMCreateCommand
        // returns the existing command if it was already created.
        unsafe {
            let command = ffi::XPLMCreateCommand(name.as_ptr(), description.as_ptr());
            if command.is_null() {
                return None;
            }
            ffi::XPLMRegisterCommandHandler(command, handler, 1, refcon);
            Some(Command {
                command,
                handler,
                refcon,
            })
        }
    }
}

impl Drop for Command {
    fn drop(&mut self) {
        // SAFETY: unregisters exactly the handler registered in `create`.
        unsafe {
            ffi::XPLMUnregisterCommandHandler(self.command, self.handler, 1, self.refcon);
        }
    }
}

/// A flight loop callback; destroyed on drop.
pub struct FlightLoop(ffi::XPLMFlightLoopID);

impl FlightLoop {
    /// Creates the flight loop and schedules its first call after `interval` seconds. The
    /// callback's return value is the delay until its next call.
    pub fn start(callback: ffi::XPLMFlightLoop_f, interval: f32) -> Option<FlightLoop> {
        let mut params = ffi::XPLMCreateFlightLoop_t {
            structSize: std::mem::size_of::<ffi::XPLMCreateFlightLoop_t>() as c_int,
            phase: ffi::xplm_FlightLoop_Phase_AfterFlightModel,
            callbackFunc: callback,
            refcon: std::ptr::null_mut(),
        };
        // SAFETY: `params` is a fully initialised struct with the size X-Plane expects.
        unsafe {
            let id = ffi::XPLMCreateFlightLoop(&mut params);
            if id.is_null() {
                return None;
            }
            ffi::XPLMScheduleFlightLoop(id, interval, 1);
            Some(FlightLoop(id))
        }
    }
}

impl Drop for FlightLoop {
    fn drop(&mut self) {
        // SAFETY: the ID came from XPLMCreateFlightLoop and is destroyed once.
        unsafe { ffi::XPLMDestroyFlightLoop(self.0) }
    }
}
