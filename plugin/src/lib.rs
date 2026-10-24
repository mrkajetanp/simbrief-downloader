//! X-Plane plugin entry points. All plugin state lives on the sim thread; downloads run on
//! a worker thread (see `worker`).

mod ffi;
mod message;
mod worker;
mod xp;

use std::cell::RefCell;
use std::ffi::{c_char, c_float, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use simbrief_core::config::Config;
use simbrief_core::matching::normalize_acf_icao;
use simbrief_core::{config_path, DownloadRequest};

use message::{Kind, MessageWindow};
use worker::Download;

const NAME: &str = "SimBrief Downloader";
const SIGNATURE: &str = "com.github.kajetan.simbriefdownloader";
const DESCRIPTION: &str =
    "Downloads SimBrief flight plans into the directory the loaded aircraft expects.";

const ACF_ICAO_DATAREF: &str = "sim/aircraft/view/acf_ICAO";
const ACF_ICAO_MAX_BYTES: usize = 40;

/// How often the flight loop checks for download results and message timeouts.
const POLL_INTERVAL: f32 = 0.25;

/// Menu item and command identifiers, passed through XPLM as integer refcons.
const ACTION_DOWNLOAD: usize = 1;
const ACTION_RELOAD_CONFIG: usize = 2;

struct Plugin {
    xplane_root: PathBuf,
    config_path: PathBuf,
    config: Config,
    acf_icao: Option<xp::DataRef>,
    download: Option<Download>,
    message: Option<MessageWindow>,
    // Held for their Drop impls, which unregister them from X-Plane.
    _menu: Option<xp::Menu>,
    _commands: Vec<xp::Command>,
    _flight_loop: Option<xp::FlightLoop>,
}

thread_local! {
    static PLUGIN: RefCell<Option<Plugin>> = const { RefCell::new(None) };
}

/// Runs a callback body, catching panics so they never unwind into X-Plane.
pub(crate) fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        xp::log("internal error: a callback panicked (details on stderr)");
        fallback
    })
}

fn with_plugin(f: impl FnOnce(&mut Plugin)) {
    PLUGIN.with(|cell| match cell.try_borrow_mut() {
        Ok(mut plugin) => {
            if let Some(plugin) = plugin.as_mut() {
                f(plugin);
            }
        }
        Err(_) => xp::log("internal error: re-entrant callback ignored"),
    });
}

impl Plugin {
    fn new() -> Plugin {
        let xplane_root = xp::system_path();
        let config_path = config_path(&xplane_root);
        let mut plugin = Plugin {
            xplane_root,
            config_path,
            config: Config::default(),
            acf_icao: xp::DataRef::find(ACF_ICAO_DATAREF),
            download: None,
            message: MessageWindow::create(),
            _menu: xp::Menu::create(
                NAME,
                &[
                    ("Download flight plan", ACTION_DOWNLOAD),
                    ("Reload config", ACTION_RELOAD_CONFIG),
                ],
                menu_handler,
            ),
            _commands: [
                (
                    "simbrief_downloader/download",
                    "Download the latest SimBrief flight plan",
                    ACTION_DOWNLOAD,
                ),
                (
                    "simbrief_downloader/reload_config",
                    "Reload the SimBrief Downloader config file",
                    ACTION_RELOAD_CONFIG,
                ),
            ]
            .into_iter()
            .filter_map(|(name, description, action)| {
                xp::Command::create(name, description, command_handler, action)
            })
            .collect(),
            _flight_loop: xp::FlightLoop::start(flight_loop, POLL_INTERVAL),
        };
        if plugin.acf_icao.is_none() {
            xp::log(&format!("dataref {ACF_ICAO_DATAREF} not found"));
        }
        if plugin.message.is_none() {
            xp::log("could not create the message window; messages go to Log.txt only");
        }
        xp::log(&format!("X-Plane root: {}", plugin.xplane_root.display()));
        plugin.load_config(false);
        plugin
    }

    fn notify(&mut self, kind: Kind, text: &str) {
        xp::log(text);
        if let Some(message) = self.message.as_mut() {
            message.show(kind, text);
        }
    }

    fn load_config(&mut self, announce: bool) {
        let shown_path = self
            .config_path
            .strip_prefix(&self.xplane_root)
            .unwrap_or(&self.config_path)
            .display()
            .to_string();
        match Config::load_or_create(&self.config_path) {
            Ok((config, errors)) => {
                self.config = config;
                xp::log(&format!(
                    "loaded {} ({} aircraft mapping(s))",
                    self.config_path.display(),
                    self.config.mappings.len()
                ));
                for error in &errors {
                    xp::log(&format!("config error, {error}"));
                }
                if !errors.is_empty() {
                    self.notify(
                        Kind::Error,
                        &format!(
                            "{shown_path} has {} error(s), see Log.txt; those lines are ignored",
                            errors.len()
                        ),
                    );
                } else if announce {
                    self.notify(Kind::Success, "SimBrief Downloader config reloaded");
                }
            }
            Err(error) => {
                self.config = Config::default();
                self.notify(
                    Kind::Error,
                    &format!("Could not read {shown_path}: {error}"),
                );
            }
        }
    }

    fn loaded_aircraft_icao(&self) -> String {
        self.acf_icao
            .as_ref()
            .map(|dataref| normalize_acf_icao(&dataref.get_bytes(ACF_ICAO_MAX_BYTES)))
            .unwrap_or_default()
    }

    fn start_download(&mut self) {
        if self.download.is_some() {
            self.notify(Kind::Info, "A SimBrief download is already in progress");
            return;
        }
        let Some(simbrief_id) = self.config.simbrief_id else {
            self.notify(
                Kind::Error,
                "Set your SimBrief pilot ID (simbrief_id) in Output/preferences/SimBriefDownloader.txt, then reload the config",
            );
            return;
        };
        let loaded_icao = self.loaded_aircraft_icao();
        xp::log(&format!(
            "fetching latest OFP for pilot ID {simbrief_id}, loaded aircraft '{loaded_icao}'"
        ));
        let request = DownloadRequest {
            simbrief_id,
            xplane_root: self.xplane_root.clone(),
            config: self.config.clone(),
            loaded_icao,
            format_override: None,
        };
        match Download::spawn(request) {
            Ok(download) => {
                self.download = Some(download);
                self.notify(Kind::Info, "Downloading flight plan from SimBrief...");
            }
            Err(error) => self.notify(
                Kind::Error,
                &format!("Could not start the download: {error}"),
            ),
        }
    }

    fn poll(&mut self) {
        if let Some(outcome) = self.download.as_ref().and_then(Download::poll) {
            self.download = None;
            match outcome {
                Ok(saved) => {
                    xp::log(&format!(
                        "saved {} ({}, format '{}') to {}",
                        saved.file_name,
                        saved.format_name,
                        saved.format,
                        saved.path.display()
                    ));
                    self.notify(
                        Kind::Success,
                        &format!("Saved {} to {}", saved.file_name, saved.display_dir),
                    );
                }
                Err(error) => {
                    if let Some(details) = error.details() {
                        xp::log(&details);
                    }
                    self.notify(Kind::Error, &error.to_string());
                }
            }
        }
        if let Some(message) = self.message.as_mut() {
            message.tick();
        }
    }

    fn run_action(&mut self, action: usize) {
        match action {
            ACTION_DOWNLOAD => self.start_download(),
            ACTION_RELOAD_CONFIG => self.load_config(true),
            _ => {}
        }
    }
}

unsafe extern "C" fn menu_handler(_menu_ref: *mut c_void, item_ref: *mut c_void) {
    guard((), || {
        with_plugin(|plugin| plugin.run_action(item_ref as usize))
    });
}

unsafe extern "C" fn command_handler(
    _command: ffi::XPLMCommandRef,
    phase: ffi::XPLMCommandPhase,
    refcon: *mut c_void,
) -> c_int {
    guard(1, || {
        if phase == ffi::xplm_CommandBegin {
            with_plugin(|plugin| plugin.run_action(refcon as usize));
        }
        1
    })
}

unsafe extern "C" fn flight_loop(
    _elapsed_since_last_call: c_float,
    _elapsed_since_last_flight_loop: c_float,
    _counter: c_int,
    _refcon: *mut c_void,
) -> c_float {
    guard(POLL_INTERVAL, || {
        with_plugin(Plugin::poll);
        POLL_INTERVAL
    })
}

/// # Safety
/// Called by X-Plane with three writable 256-byte buffers.
#[no_mangle]
pub unsafe extern "C" fn XPluginStart(
    out_name: *mut c_char,
    out_signature: *mut c_char,
    out_description: *mut c_char,
) -> c_int {
    guard(0, || {
        // SAFETY: X-Plane passes three 256-byte buffers.
        unsafe {
            xp::write_start_string(out_name, NAME);
            xp::write_start_string(out_signature, SIGNATURE);
            xp::write_start_string(out_description, DESCRIPTION);
        }
        xp::enable_native_paths();
        1
    })
}

#[no_mangle]
pub extern "C" fn XPluginStop() {}

#[no_mangle]
pub extern "C" fn XPluginEnable() -> c_int {
    guard(0, || {
        xp::log(&format!("version {} enabled", env!("CARGO_PKG_VERSION")));
        let plugin = Plugin::new();
        PLUGIN.with(|cell| *cell.borrow_mut() = Some(plugin));
        1
    })
}

#[no_mangle]
pub extern "C" fn XPluginDisable() {
    guard((), || {
        // Take the state out first so that dropping it (which unregisters callbacks) does
        // not happen while it is borrowed. A running download thread is left to finish on
        // its own; its result is discarded.
        let plugin = PLUGIN.with(|cell| cell.borrow_mut().take());
        drop(plugin);
        xp::log("disabled");
    })
}

#[no_mangle]
pub extern "C" fn XPluginReceiveMessage(_from: c_int, _message: c_int, _param: *mut c_void) {}
