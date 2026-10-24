//! SimBrief download logic for the X-Plane plugin. Nothing in this crate touches XPLM.

pub mod config;
pub mod matching;
pub mod routing;
pub mod simbrief;
pub mod write;

use std::path::{Path, PathBuf};

use config::Config;
use simbrief::{HttpClient, Ofp};

/// Everything that can go wrong in a download. `Display` gives a short message meant for
/// the on-screen notification; [`Error::details`] has extra text for the log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Could not reach SimBrief: {0}")]
    Network(String),
    #[error("SimBrief: {0}")]
    SimBrief(String),
    #[error("Unexpected response from SimBrief: {0}")]
    InvalidResponse(String),
    #[error("The flight plan does not name an aircraft type; nothing downloaded")]
    OfpNoIcao,
    #[error("The loaded aircraft does not report an ICAO type; nothing downloaded")]
    LoadedAircraftNoIcao,
    #[error("Flight plan is for {planned_icao} ({planned_name}), but the loaded aircraft is {loaded_icao}; nothing downloaded")]
    AircraftMismatch {
        planned_icao: String,
        planned_name: String,
        loaded_icao: String,
    },
    #[error("SimBrief has no '{format}' download format (see Log.txt for the available ones)")]
    FormatUnavailable {
        format: String,
        available: Vec<String>,
    },
    #[error("Directory {0} does not exist; check the config")]
    MissingDirectory(String),
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Could not write {path}: {message}")]
    Write { path: String, message: String },
}

impl Error {
    /// Extra information worth logging but too long for the screen.
    pub fn details(&self) -> Option<String> {
        match self {
            Error::FormatUnavailable { available, .. } => {
                Some(format!("Available formats: {}", available.join(", ")))
            }
            _ => None,
        }
    }
}

/// Inputs to one download, gathered on the sim thread.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub simbrief_id: u64,
    pub xplane_root: PathBuf,
    pub config: Config,
    /// ICAO type of the aircraft loaded in the sim (`acf_ICAO`).
    pub loaded_icao: String,
    /// Use this SimBrief format instead of the configured one (dev tool only; the
    /// plugin always passes `None`).
    pub format_override: Option<String>,
}

/// A flight plan that was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    pub file_name: String,
    /// Directory relative to the X-Plane root, `/`-separated.
    pub display_dir: String,
    pub aircraft_icao: String,
    pub format: String,
    pub format_name: String,
}

/// Fetches the latest OFP, checks that it is for the loaded aircraft, then downloads the
/// flight plan in the configured format and writes it to the configured directory.
/// Nothing is downloaded or written unless the aircraft check passes.
pub fn download_flight_plan(
    client: &dyn HttpClient,
    request: &DownloadRequest,
) -> Result<Saved, Error> {
    let response = client
        .get(&simbrief::ofp_url(request.simbrief_id))
        .map_err(Error::Network)?;
    let ofp = match Ofp::parse(&response.body) {
        Err(Error::InvalidResponse(_)) if response.status != 200 => {
            return Err(Error::InvalidResponse(format!("HTTP {}", response.status)))
        }
        result => result?,
    };

    let icao = matching::check_match(&request.loaded_icao, &ofp)?;
    let target = routing::resolve_target(&request.xplane_root, &request.config, &icao)?;
    let format = match &request.format_override {
        Some(format) => format.to_ascii_lowercase(),
        None => target.format.clone(),
    };
    let download = ofp.download(&format)?;
    let url = ofp.download_url(download)?;

    let file = client.get(&url).map_err(Error::Network)?;
    check_file_response(&file)?;
    let file_name =
        routing::output_file_name(&ofp, file.content_disposition.as_deref(), &download.link)?;

    let path = target.dir.join(&file_name);
    write::atomic_write(&path, &file.body).map_err(|e| Error::Write {
        path: format!("{}/{}", target.display_dir, file_name),
        message: e.to_string(),
    })?;

    Ok(Saved {
        path,
        file_name,
        display_dir: target.display_dir,
        aircraft_icao: icao,
        format,
        format_name: download.name.clone(),
    })
}

fn check_file_response(file: &simbrief::HttpResponse) -> Result<(), Error> {
    if file.status != 200 {
        return Err(Error::Download(format!("HTTP {}", file.status)));
    }
    if file.body.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::Download("SimBrief sent an empty file".into()));
    }
    let html_type = file
        .content_type
        .as_deref()
        .is_some_and(|t| t.to_ascii_lowercase().contains("text/html"));
    if html_type || looks_like_html(&file.body) {
        return Err(Error::Download(
            "SimBrief sent a web page instead of a flight plan".into(),
        ));
    }
    Ok(())
}

fn looks_like_html(body: &[u8]) -> bool {
    let start = body
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(body.len());
    let head: Vec<u8> = body[start..]
        .iter()
        .take(14)
        .map(u8::to_ascii_lowercase)
        .collect();
    head.starts_with(b"<!doctype html") || head.starts_with(b"<html")
}

/// The X-Plane-relative location of the config file.
pub fn config_path(xplane_root: &Path) -> PathBuf {
    xplane_root
        .join("Output")
        .join("preferences")
        .join("SimBriefDownloader.txt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use simbrief::HttpResponse;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const OFP: &[u8] = include_bytes!("../tests/fixtures/ofp_success.json");
    const OFP_ERROR: &[u8] = include_bytes!("../tests/fixtures/ofp_error.json");
    const FLP: &[u8] = include_bytes!("../tests/fixtures/SLLPSABE_vm5.flp");

    const OFP_URL: &str = "https://www.simbrief.com/api/xml.fetcher.php?userid=1&json=1";
    const VM5_URL: &str =
        "https://www.simbrief.com/ofp/flightplans/SLLPSABE_VMX_1792870617.flp--VM5";
    const XPE_URL: &str = "https://www.simbrief.com/ofp/flightplans/SLLPSABE_XPE_1792870617.fms";

    /// Canned responses by URL; records every request.
    #[derive(Default)]
    struct FakeClient {
        responses: HashMap<String, Result<HttpResponse, String>>,
        requests: RefCell<Vec<String>>,
    }

    impl FakeClient {
        fn with(mut self, url: &str, response: Result<HttpResponse, String>) -> Self {
            self.responses.insert(url.into(), response);
            self
        }
        fn requests(&self) -> Vec<String> {
            self.requests.borrow().clone()
        }
    }

    impl HttpClient for FakeClient {
        fn get(&self, url: &str) -> Result<HttpResponse, String> {
            self.requests.borrow_mut().push(url.into());
            self.responses
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err(format!("unexpected request {url}")))
        }
    }

    fn ok(status: u16, body: &[u8], disposition: Option<&str>) -> Result<HttpResponse, String> {
        Ok(HttpResponse {
            status,
            content_type: Some("application/octet-stream".into()),
            content_disposition: disposition.map(str::to_string),
            body: body.to_vec(),
        })
    }

    fn fixture_client() -> FakeClient {
        FakeClient::default()
            .with(OFP_URL, ok(200, OFP, None))
            .with(
                VM5_URL,
                ok(200, FLP, Some("attachment; filename=\"SLLPSABE.flp\"")),
            )
            .with(
                XPE_URL,
                ok(
                    200,
                    b"I\n1100 Version\n",
                    Some("attachment; filename=\"SLLPSABE.fms\""),
                ),
            )
    }

    /// X-Plane root with an `Aircraft/Embraer/co-routes` directory.
    fn xplane_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            root.path()
                .join("Aircraft")
                .join("Embraer")
                .join("co-routes"),
        )
        .unwrap();
        root
    }

    fn request(root: &Path, config: &str, loaded: &str) -> DownloadRequest {
        let (config, errors) = Config::parse(config);
        assert!(errors.is_empty());
        DownloadRequest {
            simbrief_id: 1,
            xplane_root: root.to_path_buf(),
            config,
            loaded_icao: loaded.into(),
            format_override: None,
        }
    }

    #[test]
    fn mapped_aircraft_gets_its_format_in_its_directory() {
        let root = xplane_root();
        let client = fixture_client();
        let saved = download_flight_plan(
            &client,
            &request(root.path(), "E55P:Embraer/co-routes:vm5", "E55P"),
        )
        .unwrap();

        let expected = root
            .path()
            .join("Aircraft")
            .join("Embraer")
            .join("co-routes")
            .join("SLLPSABE.flp");
        assert_eq!(saved.path, expected);
        assert_eq!(saved.file_name, "SLLPSABE.flp");
        assert_eq!(saved.display_dir, "Aircraft/Embraer/co-routes");
        assert_eq!(saved.format_name, "FlightFactor 757 V2");
        assert_eq!(std::fs::read(&expected).unwrap(), FLP);
        assert_eq!(client.requests(), vec![OFP_URL, VM5_URL]);
    }

    #[test]
    fn unmapped_aircraft_gets_xpe_in_fms_plans() {
        let root = xplane_root();
        let saved =
            download_flight_plan(&fixture_client(), &request(root.path(), "", "e55p")).unwrap();
        assert_eq!(
            saved.path,
            root.path()
                .join("Output")
                .join("FMS plans")
                .join("SLLPSABE.fms")
        );
        assert_eq!(saved.format, "xpe");
    }

    #[test]
    fn mismatch_downloads_and_writes_nothing() {
        let root = xplane_root();
        let client = fixture_client();
        let result = download_flight_plan(&client, &request(root.path(), "", "B752"));
        assert!(matches!(result, Err(Error::AircraftMismatch { .. })));
        assert_eq!(client.requests(), vec![OFP_URL]);
        assert!(!root.path().join("Output").exists());
    }

    #[test]
    fn simbrief_error_is_reported_from_http_400() {
        let root = xplane_root();
        let client = FakeClient::default().with(OFP_URL, ok(400, OFP_ERROR, None));
        assert_eq!(
            download_flight_plan(&client, &request(root.path(), "", "E55P")),
            Err(Error::SimBrief("Error: Unknown UserID".into()))
        );
    }

    #[test]
    fn non_json_error_page_reports_http_status() {
        let root = xplane_root();
        let client = FakeClient::default().with(OFP_URL, ok(503, b"<html>down</html>", None));
        assert_eq!(
            download_flight_plan(&client, &request(root.path(), "", "E55P")),
            Err(Error::InvalidResponse("HTTP 503".into()))
        );
    }

    #[test]
    fn network_failure() {
        let root = xplane_root();
        let client = FakeClient::default().with(OFP_URL, Err("timed out".into()));
        assert_eq!(
            download_flight_plan(&client, &request(root.path(), "", "E55P")),
            Err(Error::Network("timed out".into()))
        );
    }

    #[test]
    fn missing_mapped_directory_downloads_nothing() {
        let root = xplane_root();
        let client = fixture_client();
        let result = download_flight_plan(
            &client,
            &request(root.path(), "E55P:Gone/co-routes:vm5", "E55P"),
        );
        assert_eq!(
            result,
            Err(Error::MissingDirectory("Aircraft/Gone/co-routes".into()))
        );
        assert_eq!(client.requests(), vec![OFP_URL]);
    }

    #[test]
    fn unknown_format_downloads_nothing() {
        let root = xplane_root();
        let client = fixture_client();
        let result = download_flight_plan(
            &client,
            &request(root.path(), "E55P:Embraer/co-routes:abc", "E55P"),
        );
        let error = result.unwrap_err();
        assert!(matches!(error, Error::FormatUnavailable { .. }));
        assert!(error.details().unwrap().contains("vm5"));
        assert_eq!(client.requests(), vec![OFP_URL]);
    }

    #[test]
    fn html_or_empty_file_is_not_written() {
        for (body, content_type) in [
            (&b"<!DOCTYPE HTML PUBLIC>"[..], "application/octet-stream"),
            (b"plan", "text/html; charset=iso-8859-1"),
            (b"  \n", "application/octet-stream"),
        ] {
            let root = xplane_root();
            let client = fixture_client().with(
                XPE_URL,
                Ok(HttpResponse {
                    status: 200,
                    content_type: Some(content_type.into()),
                    content_disposition: None,
                    body: body.to_vec(),
                }),
            );
            let result = download_flight_plan(&client, &request(root.path(), "", "E55P"));
            assert!(matches!(result, Err(Error::Download(_))), "{result:?}");
            let plans = root.path().join("Output").join("FMS plans");
            assert_eq!(std::fs::read_dir(plans).unwrap().count(), 0);
        }
    }

    #[test]
    fn format_override_replaces_configured_format() {
        let root = xplane_root();
        let mut request = request(root.path(), "E55P:Embraer/co-routes:xpe", "E55P");
        request.format_override = Some("VM5".into());
        let saved = download_flight_plan(&fixture_client(), &request).unwrap();
        assert_eq!(saved.format, "vm5");
        assert_eq!(saved.file_name, "SLLPSABE.flp");
        assert_eq!(saved.display_dir, "Aircraft/Embraer/co-routes");
    }

    #[test]
    fn existing_plan_is_overwritten() {
        let root = xplane_root();
        let plans = root.path().join("Output").join("FMS plans");
        std::fs::create_dir_all(&plans).unwrap();
        std::fs::write(plans.join("SLLPSABE.fms"), b"old").unwrap();
        download_flight_plan(&fixture_client(), &request(root.path(), "", "E55P")).unwrap();
        assert_eq!(
            std::fs::read(plans.join("SLLPSABE.fms")).unwrap(),
            b"I\n1100 Version\n"
        );
    }
}
