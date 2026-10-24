//! Raw declarations of the XPLM functions the plugin uses, checked against the X-Plane 12
//! SDK headers. Linking: on Linux the symbols resolve when X-Plane loads the plugin, on
//! macOS via `-undefined dynamic_lookup` (see build.rs), on Windows through `raw-dylib`,
//! so no SDK files are needed to build.

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_float, c_int, c_void};

pub type XPLMDataRef = *mut c_void;
pub type XPLMMenuID = *mut c_void;
pub type XPLMCommandRef = *mut c_void;
pub type XPLMFlightLoopID = *mut c_void;
pub type XPLMWindowID = *mut c_void;
pub type XPLMFontID = c_int;
pub type XPLMCommandPhase = c_int;
pub type XPLMMouseStatus = c_int;
pub type XPLMKeyFlags = c_int;
pub type XPLMCursorStatus = c_int;

pub const xplm_CommandBegin: XPLMCommandPhase = 0;
pub const xplm_FlightLoop_Phase_AfterFlightModel: c_int = 1;
pub const xplm_WindowLayerGrowlNotifications: c_int = 3;
pub const xplm_WindowDecorationNone: c_int = 0;
pub const xplm_CursorDefault: XPLMCursorStatus = 0;
pub const xplmFont_Proportional: XPLMFontID = 18;

pub type XPLMMenuHandler_f = unsafe extern "C" fn(menu_ref: *mut c_void, item_ref: *mut c_void);
pub type XPLMCommandCallback_f = unsafe extern "C" fn(
    command: XPLMCommandRef,
    phase: XPLMCommandPhase,
    refcon: *mut c_void,
) -> c_int;
pub type XPLMFlightLoop_f = unsafe extern "C" fn(
    elapsed_since_last_call: c_float,
    elapsed_since_last_flight_loop: c_float,
    counter: c_int,
    refcon: *mut c_void,
) -> c_float;
pub type XPLMDrawWindow_f = unsafe extern "C" fn(window: XPLMWindowID, refcon: *mut c_void);
pub type XPLMHandleMouseClick_f = unsafe extern "C" fn(
    window: XPLMWindowID,
    x: c_int,
    y: c_int,
    mouse: XPLMMouseStatus,
    refcon: *mut c_void,
) -> c_int;
pub type XPLMHandleKey_f = unsafe extern "C" fn(
    window: XPLMWindowID,
    key: c_char,
    flags: XPLMKeyFlags,
    virtual_key: c_char,
    refcon: *mut c_void,
    losing_focus: c_int,
);
pub type XPLMHandleCursor_f = unsafe extern "C" fn(
    window: XPLMWindowID,
    x: c_int,
    y: c_int,
    refcon: *mut c_void,
) -> XPLMCursorStatus;
pub type XPLMHandleMouseWheel_f = unsafe extern "C" fn(
    window: XPLMWindowID,
    x: c_int,
    y: c_int,
    wheel: c_int,
    clicks: c_int,
    refcon: *mut c_void,
) -> c_int;

#[repr(C)]
pub struct XPLMCreateFlightLoop_t {
    pub structSize: c_int,
    pub phase: c_int,
    pub callbackFunc: XPLMFlightLoop_f,
    pub refcon: *mut c_void,
}

/// XPLM301 layout.
#[repr(C)]
pub struct XPLMCreateWindow_t {
    pub structSize: c_int,
    pub left: c_int,
    pub top: c_int,
    pub right: c_int,
    pub bottom: c_int,
    pub visible: c_int,
    pub drawWindowFunc: XPLMDrawWindow_f,
    pub handleMouseClickFunc: XPLMHandleMouseClick_f,
    pub handleKeyFunc: XPLMHandleKey_f,
    pub handleCursorFunc: XPLMHandleCursor_f,
    pub handleMouseWheelFunc: XPLMHandleMouseWheel_f,
    pub refcon: *mut c_void,
    pub decorateAsFloatingWindow: c_int,
    pub layer: c_int,
    pub handleRightClickFunc: XPLMHandleMouseClick_f,
}

#[cfg_attr(windows, link(name = "XPLM_64", kind = "raw-dylib"))]
extern "C" {
    // XPLMUtilities
    pub fn XPLMDebugString(string: *const c_char);
    pub fn XPLMGetSystemPath(out_path: *mut c_char);
    pub fn XPLMEnableFeature(feature: *const c_char, enable: c_int);
    pub fn XPLMCreateCommand(name: *const c_char, description: *const c_char) -> XPLMCommandRef;
    pub fn XPLMRegisterCommandHandler(
        command: XPLMCommandRef,
        handler: XPLMCommandCallback_f,
        before: c_int,
        refcon: *mut c_void,
    );
    pub fn XPLMUnregisterCommandHandler(
        command: XPLMCommandRef,
        handler: XPLMCommandCallback_f,
        before: c_int,
        refcon: *mut c_void,
    );

    // XPLMDataAccess
    pub fn XPLMFindDataRef(name: *const c_char) -> XPLMDataRef;
    pub fn XPLMGetDatab(
        data_ref: XPLMDataRef,
        out_value: *mut c_void,
        offset: c_int,
        max_bytes: c_int,
    ) -> c_int;

    // XPLMMenus
    pub fn XPLMFindPluginsMenu() -> XPLMMenuID;
    pub fn XPLMCreateMenu(
        name: *const c_char,
        parent_menu: XPLMMenuID,
        parent_item: c_int,
        handler: XPLMMenuHandler_f,
        menu_ref: *mut c_void,
    ) -> XPLMMenuID;
    pub fn XPLMDestroyMenu(menu: XPLMMenuID);
    pub fn XPLMAppendMenuItem(
        menu: XPLMMenuID,
        name: *const c_char,
        item_ref: *mut c_void,
        deprecated_and_ignored: c_int,
    ) -> c_int;
    pub fn XPLMRemoveMenuItem(menu: XPLMMenuID, index: c_int);

    // XPLMProcessing
    pub fn XPLMCreateFlightLoop(params: *mut XPLMCreateFlightLoop_t) -> XPLMFlightLoopID;
    pub fn XPLMDestroyFlightLoop(flight_loop: XPLMFlightLoopID);
    pub fn XPLMScheduleFlightLoop(
        flight_loop: XPLMFlightLoopID,
        interval: c_float,
        relative_to_now: c_int,
    );

    // XPLMDisplay
    pub fn XPLMCreateWindowEx(params: *mut XPLMCreateWindow_t) -> XPLMWindowID;
    pub fn XPLMDestroyWindow(window: XPLMWindowID);
    pub fn XPLMGetScreenBoundsGlobal(
        left: *mut c_int,
        top: *mut c_int,
        right: *mut c_int,
        bottom: *mut c_int,
    );
    pub fn XPLMGetWindowGeometry(
        window: XPLMWindowID,
        left: *mut c_int,
        top: *mut c_int,
        right: *mut c_int,
        bottom: *mut c_int,
    );
    pub fn XPLMSetWindowGeometry(
        window: XPLMWindowID,
        left: c_int,
        top: c_int,
        right: c_int,
        bottom: c_int,
    );
    pub fn XPLMSetWindowIsVisible(window: XPLMWindowID, visible: c_int);

    // XPLMGraphics
    pub fn XPLMDrawTranslucentDarkBox(left: c_int, top: c_int, right: c_int, bottom: c_int);
    pub fn XPLMDrawString(
        color_rgb: *mut c_float,
        x: c_int,
        y: c_int,
        string: *const c_char,
        word_wrap_width: *mut c_int,
        font: XPLMFontID,
    );
    pub fn XPLMMeasureString(font: XPLMFontID, string: *const c_char, num_chars: c_int) -> c_float;
    pub fn XPLMGetFontDimensions(
        font: XPLMFontID,
        char_width: *mut c_int,
        char_height: *mut c_int,
        digits_only: *mut c_int,
    );
}
