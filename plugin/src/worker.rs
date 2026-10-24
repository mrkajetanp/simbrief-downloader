//! Runs a download on a background thread so the sim never waits on the network.
//! The sim thread polls for the result; XPLM is never called from the worker.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use simbrief_core::simbrief::UreqClient;
use simbrief_core::{download_flight_plan, DownloadRequest, Error, Saved};

const USER_AGENT: &str = concat!("SimBriefDownloader/", env!("CARGO_PKG_VERSION"));

pub type Outcome = Result<Saved, Error>;

pub struct Download {
    receiver: Receiver<Outcome>,
}

impl Download {
    pub fn spawn(request: DownloadRequest) -> std::io::Result<Download> {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("simbrief-download".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    download_flight_plan(&UreqClient::new(USER_AGENT), &request)
                }))
                .unwrap_or_else(|_| Err(Error::Download("internal error (panic)".into())));
                // The plugin may have been disabled meanwhile; then nobody is listening.
                let _ = sender.send(outcome);
            })?;
        Ok(Download { receiver })
    }

    /// The outcome if the download has finished.
    pub fn poll(&self) -> Option<Outcome> {
        match self.receiver.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Error::Download(
                "download thread stopped unexpectedly".into(),
            ))),
        }
    }
}
