//! SimBrief API: request URLs, the parts of the OFP response we use, and HTTP access.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::Error;

/// URL of the latest OFP for a pilot, as JSON.
pub fn ofp_url(simbrief_id: u64) -> String {
    format!("https://www.simbrief.com/api/xml.fetcher.php?userid={simbrief_id}&json=1")
}

/// One entry of `fms_downloads`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Human-readable format name, e.g. "FlightFactor 757 V2".
    pub name: String,
    /// File link relative to the downloads directory, e.g. `SLLPSABE_VMX_1792870617.flp--VM5`.
    pub link: String,
}

/// The fields of a SimBrief OFP that the plugin uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ofp {
    pub request_id: Option<String>,
    /// `aircraft.icaocode`; empty if SimBrief did not send one.
    pub aircraft_icao: String,
    pub aircraft_name: String,
    pub origin: String,
    pub destination: String,
    /// `fms_downloads.directory`, the base URL for download links.
    pub downloads_directory: String,
    /// `fms_downloads` entries keyed by format, e.g. `xpe`, `vm5`.
    pub downloads: BTreeMap<String, Download>,
}

impl Ofp {
    /// Parses a fetcher response body. A `fetch.status` other than `Success` becomes
    /// [`Error::SimBrief`] carrying SimBrief's message.
    ///
    /// Fields are read from untyped JSON because SimBrief converts its XML to JSON and
    /// empty elements come through as `{}` instead of strings.
    pub fn parse(body: &[u8]) -> Result<Ofp, Error> {
        let json: Value = serde_json::from_slice(body)
            .map_err(|e| Error::InvalidResponse(format!("not valid JSON ({e})")))?;

        let status = text(&json, &["fetch", "status"]);
        if status != "Success" {
            let message = if status.is_empty() {
                "no status in response".to_string()
            } else {
                status
            };
            return Err(Error::SimBrief(message));
        }

        let mut downloads = BTreeMap::new();
        let mut downloads_directory = String::new();
        if let Some(entries) = json.get("fms_downloads").and_then(Value::as_object) {
            for (key, value) in entries {
                if key == "directory" {
                    downloads_directory = value.as_str().unwrap_or_default().to_string();
                    continue;
                }
                let link = text(value, &["link"]);
                if link.is_empty() {
                    continue;
                }
                downloads.insert(
                    key.to_ascii_lowercase(),
                    Download {
                        name: text(value, &["name"]),
                        link,
                    },
                );
            }
        }

        let request_id = Some(text(&json, &["params", "request_id"])).filter(|s| !s.is_empty());

        Ok(Ofp {
            request_id,
            aircraft_icao: text(&json, &["aircraft", "icaocode"]),
            aircraft_name: text(&json, &["aircraft", "name"]),
            origin: text(&json, &["origin", "icao_code"]),
            destination: text(&json, &["destination", "icao_code"]),
            downloads_directory,
            downloads,
        })
    }

    /// Looks up a download format. The error lists the formats that are available.
    pub fn download(&self, format: &str) -> Result<&Download, Error> {
        self.downloads
            .get(format)
            .ok_or_else(|| Error::FormatUnavailable {
                format: format.to_string(),
                available: self.downloads.keys().cloned().collect(),
            })
    }

    /// Full URL of a download: `directory + link`, verbatim. SimBrief redirects it to the
    /// file itself.
    pub fn download_url(&self, download: &Download) -> Result<String, Error> {
        if !self.downloads_directory.starts_with("https://") {
            return Err(Error::InvalidResponse(format!(
                "unexpected downloads directory '{}'",
                self.downloads_directory
            )));
        }
        Ok(format!("{}{}", self.downloads_directory, download.link))
    }
}

/// String at a JSON path, trimmed; empty if missing or not a string.
fn text(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |v, key| v.get(key))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The parts of an HTTP response the download flow looks at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
    pub body: Vec<u8>,
}

/// HTTP GET, behind a trait so that tests can use canned responses.
/// Implementations must follow redirects and must not treat 4xx/5xx as errors
/// (SimBrief reports API errors as JSON with status 400).
pub trait HttpClient {
    fn get(&self, url: &str) -> Result<HttpResponse, String>;
}

/// Responses larger than this are rejected; real OFPs are a few hundred KB.
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

pub struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    pub fn new(user_agent: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(user_agent)
            .build();
        UreqClient {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl HttpClient for UreqClient {
    fn get(&self, url: &str) -> Result<HttpResponse, String> {
        let mut response = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type");
        let content_disposition = header("content-disposition");
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok(HttpResponse {
            status,
            content_type,
            content_disposition,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: &[u8] = include_bytes!("../tests/fixtures/ofp_success.json");
    const ERROR: &[u8] = include_bytes!("../tests/fixtures/ofp_error.json");

    #[test]
    fn parses_success_fixture() {
        let ofp = Ofp::parse(SUCCESS).unwrap();
        assert_eq!(ofp.request_id.as_deref(), Some("191000981"));
        assert_eq!(ofp.aircraft_icao, "E55P");
        assert_eq!(ofp.aircraft_name, "Phenom 300");
        assert_eq!(ofp.origin, "SLLP");
        assert_eq!(ofp.destination, "SABE");
        assert_eq!(
            ofp.downloads["vm5"],
            Download {
                name: "FlightFactor 757 V2".into(),
                link: "SLLPSABE_VMX_1792870617.flp--VM5".into(),
            }
        );
        assert!(!ofp.downloads.contains_key("directory"));
    }

    #[test]
    fn download_url_uses_link_verbatim() {
        let ofp = Ofp::parse(SUCCESS).unwrap();
        let zbo = ofp.download("zbo").unwrap();
        assert_eq!(
            ofp.download_url(zbo).unwrap(),
            "https://www.simbrief.com/ofp/flightplans/xml/SLLPSABE_XML_1792870617.xml--ZBO"
        );
    }

    #[test]
    fn error_status_is_reported() {
        assert_eq!(
            Ofp::parse(ERROR),
            Err(Error::SimBrief("Error: Unknown UserID".into()))
        );
    }

    #[test]
    fn missing_format_lists_available() {
        let ofp = Ofp::parse(SUCCESS).unwrap();
        match ofp.download("abc") {
            Err(Error::FormatUnavailable { format, available }) => {
                assert_eq!(format, "abc");
                assert!(available.contains(&"xpe".to_string()));
                assert!(available.contains(&"vm5".to_string()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn invalid_json_and_missing_status() {
        assert!(matches!(
            Ofp::parse(b"<html>oops</html>"),
            Err(Error::InvalidResponse(_))
        ));
        assert_eq!(
            Ofp::parse(b"{}"),
            Err(Error::SimBrief("no status in response".into()))
        );
    }

    #[test]
    fn empty_xml_elements_become_empty_strings() {
        let body = br#"{"fetch":{"status":"Success"},"aircraft":{"icaocode":{}},
            "params":{"request_id":{}},"fms_downloads":{"directory":"https://x/","xpe":{"name":"X","link":{}}}}"#;
        let ofp = Ofp::parse(body).unwrap();
        assert_eq!(ofp.aircraft_icao, "");
        assert_eq!(ofp.request_id, None);
        assert!(ofp.downloads.is_empty());
    }

    #[test]
    fn ofp_url_format() {
        assert_eq!(
            ofp_url(123456),
            "https://www.simbrief.com/api/xml.fetcher.php?userid=123456&json=1"
        );
    }
}
