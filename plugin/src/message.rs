//! A small notification window at the top of the screen that hides itself after a few
//! seconds, or when clicked.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::time::{Duration, Instant};

use crate::ffi;
use crate::xp::c_string;

const SHOW_FOR: Duration = Duration::from_secs(6);
const PADDING: c_int = 12;
const TOP_OFFSET: c_int = 60;
const FONT: ffi::XPLMFontID = ffi::xplmFont_Proportional;

#[derive(Clone, Copy)]
pub enum Kind {
    Info,
    Success,
    Error,
}

impl Kind {
    fn color(self) -> [f32; 3] {
        match self {
            Kind::Info => [0.9, 0.9, 0.9],
            Kind::Success => [0.55, 1.0, 0.55],
            Kind::Error => [1.0, 0.55, 0.45],
        }
    }
}

struct Content {
    text: CString,
    color: [f32; 3],
    /// Word wrap width in boxels, 0 for none.
    wrap: c_int,
}

pub struct MessageWindow {
    window: ffi::XPLMWindowID,
    /// Owned here and passed to X-Plane as the window refcon; must outlive the window.
    content: Box<RefCell<Content>>,
    hide_at: Option<Instant>,
}

impl MessageWindow {
    pub fn create() -> Option<MessageWindow> {
        let content = Box::new(RefCell::new(Content {
            text: CString::default(),
            color: Kind::Info.color(),
            wrap: 0,
        }));
        let mut params = ffi::XPLMCreateWindow_t {
            structSize: std::mem::size_of::<ffi::XPLMCreateWindow_t>() as c_int,
            left: 0,
            top: 100,
            right: 100,
            bottom: 0,
            visible: 0,
            drawWindowFunc: draw,
            handleMouseClickFunc: click,
            handleKeyFunc: key,
            handleCursorFunc: cursor,
            handleMouseWheelFunc: wheel,
            refcon: (&*content as *const RefCell<Content>).cast_mut().cast(),
            decorateAsFloatingWindow: ffi::xplm_WindowDecorationNone,
            layer: ffi::xplm_WindowLayerGrowlNotifications,
            handleRightClickFunc: click,
        };
        // SAFETY: `params` is fully initialised with the XPLM301 layout; the refcon points
        // into a Box that lives as long as the window (destroyed first in Drop).
        let window = unsafe { ffi::XPLMCreateWindowEx(&mut params) };
        (!window.is_null()).then_some(MessageWindow {
            window,
            content,
            hide_at: None,
        })
    }

    pub fn show(&mut self, kind: Kind, text: &str) {
        let text = c_string(text);
        let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
        let mut line_height = 0;
        // SAFETY: plain value queries with valid out-pointers; `text` is NUL-terminated.
        let text_width = unsafe {
            ffi::XPLMGetScreenBoundsGlobal(&mut left, &mut top, &mut right, &mut bottom);
            ffi::XPLMGetFontDimensions(
                FONT,
                std::ptr::null_mut(),
                &mut line_height,
                std::ptr::null_mut(),
            );
            let len = c_int::try_from(text.as_bytes().len()).unwrap_or(c_int::MAX);
            ffi::XPLMMeasureString(FONT, text.as_ptr(), len).ceil() as c_int
        };
        let line_height = if line_height > 0 { line_height } else { 14 };

        let max_text_width = (right - left - 4 * PADDING).max(200);
        let (wrap, lines) = if text_width > max_text_width {
            (max_text_width, text_width / max_text_width + 1)
        } else {
            (0, 1)
        };
        let width = text_width.min(max_text_width) + 2 * PADDING;
        let height = lines * (line_height + 4) + 2 * PADDING;
        let window_left = left + (right - left - width) / 2;
        let window_top = top - TOP_OFFSET;

        *self.content.borrow_mut() = Content {
            text,
            color: kind.color(),
            wrap,
        };
        // SAFETY: the window handle is valid until Drop.
        unsafe {
            ffi::XPLMSetWindowGeometry(
                self.window,
                window_left,
                window_top,
                window_left + width,
                window_top - height,
            );
            ffi::XPLMSetWindowIsVisible(self.window, 1);
        }
        self.hide_at = Some(Instant::now() + SHOW_FOR);
    }

    /// Hides the message once its time is up. Call regularly from the flight loop.
    pub fn tick(&mut self) {
        if self.hide_at.is_some_and(|at| Instant::now() >= at) {
            self.hide_at = None;
            // SAFETY: the window handle is valid until Drop.
            unsafe { ffi::XPLMSetWindowIsVisible(self.window, 0) }
        }
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        // SAFETY: destroys the window before `content` (its refcon) is freed.
        unsafe { ffi::XPLMDestroyWindow(self.window) }
    }
}

/// Runs `f` on the window content behind a refcon, ignoring panics (logged by the hook).
fn with_content(refcon: *mut c_void, f: impl FnOnce(&Content)) {
    // SAFETY: the refcon is the `RefCell<Content>` owned by the live MessageWindow.
    let content = unsafe { &*(refcon as *const RefCell<Content>) };
    if let Ok(content) = content.try_borrow() {
        f(&content);
    }
}

unsafe extern "C" fn draw(window: ffi::XPLMWindowID, refcon: *mut c_void) {
    crate::guard((), || {
        with_content(refcon, |content| {
            let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
            let mut color = content.color;
            let mut wrap = content.wrap;
            let wrap_ptr = if wrap > 0 {
                &mut wrap as *mut c_int
            } else {
                std::ptr::null_mut()
            };
            // SAFETY: valid window handle and out-pointers; X-Plane does not keep pointers.
            unsafe {
                ffi::XPLMGetWindowGeometry(window, &mut left, &mut top, &mut right, &mut bottom);
                ffi::XPLMDrawTranslucentDarkBox(left, top, right, bottom);
                let mut line_height = 0;
                ffi::XPLMGetFontDimensions(
                    FONT,
                    std::ptr::null_mut(),
                    &mut line_height,
                    std::ptr::null_mut(),
                );
                let baseline = top - PADDING - line_height.max(10);
                ffi::XPLMDrawString(
                    color.as_mut_ptr(),
                    left + PADDING,
                    baseline,
                    content.text.as_ptr(),
                    wrap_ptr,
                    FONT,
                );
            }
        })
    })
}

unsafe extern "C" fn click(
    window: ffi::XPLMWindowID,
    _x: c_int,
    _y: c_int,
    _mouse: ffi::XPLMMouseStatus,
    _refcon: *mut c_void,
) -> c_int {
    // SAFETY: X-Plane only calls this for our live window.
    unsafe { ffi::XPLMSetWindowIsVisible(window, 0) };
    1
}

unsafe extern "C" fn key(
    _window: ffi::XPLMWindowID,
    _key: c_char,
    _flags: ffi::XPLMKeyFlags,
    _virtual_key: c_char,
    _refcon: *mut c_void,
    _losing_focus: c_int,
) {
}

unsafe extern "C" fn cursor(
    _window: ffi::XPLMWindowID,
    _x: c_int,
    _y: c_int,
    _refcon: *mut c_void,
) -> ffi::XPLMCursorStatus {
    ffi::xplm_CursorDefault
}

unsafe extern "C" fn wheel(
    _window: ffi::XPLMWindowID,
    _x: c_int,
    _y: c_int,
    _wheel: c_int,
    _clicks: c_int,
    _refcon: *mut c_void,
) -> c_int {
    0
}
