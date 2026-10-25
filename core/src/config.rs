//! Parsing of the `SimBriefDownloader.txt` config file.
//!
//! Format: one entry per line, `#` comments and blank lines ignored.
//! `simbrief_id:<n>` sets the pilot ID, every other line is `ICAO:path[:format]`.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The SimBrief download format used when a mapping does not name one (X-Plane 11/12 `.fms`).
pub const DEFAULT_FORMAT: &str = "xpe";

const SIMBRIEF_ID_KEY: &str = "simbrief_id";

/// Written to disk when the config file does not exist yet.
pub const DEFAULT_CONFIG: &str = "\
# SimBrief Downloader configuration
#
# SimBrief pilot ID (numeric, from https://dispatch.simbrief.com/account)
simbrief_id:

# Aircraft-specific flight plan directories, one per line:
#
#   ICAO:path[:format]
#
#   ICAO   - aircraft type designator as used by SimBrief and X-Plane, e.g. B752
#   path   - directory relative to <X-Plane>/Aircraft/; leave it empty for
#            the default Output/FMS plans directory
#   format - optional SimBrief download format (default: xpe = X-Plane 11/12 .fms)
#
# Aircraft without an entry get an X-Plane .fms file in Output/FMS plans.
#
# Zibo 737-800 reads SimBrief's Zibo XML format from Output/FMS plans/b738x.xml:
B738::zbo
#
# FlightFactor 757 V2 reads its own .flp route format from co-routes:
#B752:Boeing/757/co-routes:vm5
#B753:Boeing/757/co-routes:vm5
";

/// Where flight plans for one aircraft type go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Directory relative to `<X-Plane>/Aircraft/`, already validated. `None` (an empty
    /// path in the file) means the default `Output/FMS plans` directory.
    pub path: Option<PathBuf>,
    /// SimBrief `fms_downloads` key, lowercase.
    pub format: String,
    /// 1-based line number in the config file.
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub simbrief_id: Option<u64>,
    /// Keyed by uppercase ICAO type designator.
    pub mappings: HashMap<String, Mapping>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl Config {
    /// Parses the config text. Invalid lines are reported and skipped; the valid entries
    /// are still returned so that one bad line does not disable the plugin.
    pub fn parse(text: &str) -> (Config, Vec<ConfigError>) {
        let mut config = Config::default();
        let mut errors = Vec::new();
        let mut id_line: Option<usize> = None;

        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let content = raw.trim();
            if content.is_empty() || content.starts_with('#') {
                continue;
            }
            let mut err = |message: String| errors.push(ConfigError { line, message });

            let fields: Vec<&str> = content.split(':').map(str::trim).collect();
            let key = fields[0];

            if key.eq_ignore_ascii_case(SIMBRIEF_ID_KEY) {
                if fields.len() != 2 {
                    err("expected simbrief_id:<number>".into());
                    continue;
                }
                if let Some(previous) = id_line {
                    err(format!("simbrief_id is already set on line {previous}"));
                    continue;
                }
                id_line = Some(line);
                // An empty value (as in the default file) means "not set yet".
                if fields[1].is_empty() {
                    continue;
                }
                match fields[1].parse::<u64>() {
                    Ok(id) if id > 0 => config.simbrief_id = Some(id),
                    _ => err(format!(
                        "simbrief_id must be a positive number, got '{}'",
                        fields[1]
                    )),
                }
                continue;
            }

            if fields.len() < 2 || fields.len() > 3 {
                err(format!(
                    "expected ICAO:path or ICAO:path:format, got {} field(s)",
                    fields.len()
                ));
                continue;
            }

            let icao = match validate_icao(key) {
                Ok(icao) => icao,
                Err(message) => {
                    err(message);
                    continue;
                }
            };
            let path = match fields[1] {
                "" => None,
                raw => match validate_aircraft_path(raw) {
                    Ok(path) => Some(path),
                    Err(message) => {
                        err(message);
                        continue;
                    }
                },
            };
            let format = match fields.get(2) {
                None => DEFAULT_FORMAT.to_string(),
                Some(format) => match validate_format(format) {
                    Ok(format) => format,
                    Err(message) => {
                        err(message);
                        continue;
                    }
                },
            };

            if let Some(existing) = config.mappings.get(&icao) {
                err(format!(
                    "{icao} is already mapped on line {}",
                    existing.line
                ));
                continue;
            }
            config.mappings.insert(icao, Mapping { path, format, line });
        }

        (config, errors)
    }

    /// Reads the config file, first creating it with [`DEFAULT_CONFIG`] if it does not exist.
    pub fn load_or_create(path: &Path) -> std::io::Result<(Config, Vec<ConfigError>)> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::write::atomic_write(path, DEFAULT_CONFIG.as_bytes())?;
        }
        let text = std::fs::read_to_string(path)?;
        Ok(Config::parse(&text))
    }
}

fn validate_icao(key: &str) -> Result<String, String> {
    if key.is_empty() {
        return Err("missing ICAO type before ':'".into());
    }
    if key.len() > 4 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "'{key}' is not an ICAO aircraft type (expected up to 4 letters/digits)"
        ));
    }
    Ok(key.to_ascii_uppercase())
}

fn validate_format(format: &str) -> Result<String, String> {
    if format.is_empty() {
        return Err("format after the second ':' is empty".into());
    }
    if !format.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "format '{format}' must contain only letters and digits"
        ));
    }
    Ok(format.to_ascii_lowercase())
}

/// Validates a directory relative to `Aircraft/`. Accepts `/` and `\` as separators and
/// rejects anything that could point outside `Aircraft/`.
pub fn validate_aircraft_path(raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("path is empty".into());
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err(format!(
            "path '{raw}' must be relative to the Aircraft directory, not absolute"
        ));
    }
    let mut path = PathBuf::new();
    for component in raw.split(['/', '\\']) {
        match component.trim() {
            "" | "." => continue,
            ".." => return Err(format!("path '{raw}' must not contain '..'")),
            part => path.push(part),
        }
    }
    if path.as_os_str().is_empty() {
        return Err(format!("path '{raw}' does not name a directory"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> Config {
        let (config, errors) = Config::parse(text);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        config
    }

    fn error_lines(text: &str) -> Vec<usize> {
        Config::parse(text).1.iter().map(|e| e.line).collect()
    }

    #[test]
    fn parses_id_and_mappings() {
        let config = parse_ok(
            "# comment\n\n  simbrief_id : 123456 \nB752:Boeing/757/co-routes:vm5\nb738 : Zibo/B738 \n",
        );
        assert_eq!(config.simbrief_id, Some(123456));
        assert_eq!(
            config.mappings["B752"],
            Mapping {
                path: Some(PathBuf::from("Boeing").join("757").join("co-routes")),
                format: "vm5".into(),
                line: 4,
            }
        );
        let b738 = &config.mappings["B738"];
        assert_eq!(b738.path, Some(PathBuf::from("Zibo").join("B738")));
        assert_eq!(b738.format, DEFAULT_FORMAT);
    }

    #[test]
    fn default_config_parses_cleanly() {
        let config = parse_ok(DEFAULT_CONFIG);
        assert_eq!(config.simbrief_id, None);
        assert_eq!(config.mappings.len(), 1);
        let b738 = &config.mappings["B738"];
        assert_eq!(b738.path, None);
        assert_eq!(b738.format, "zbo");
    }

    #[test]
    fn empty_path_means_default_directory() {
        let config = parse_ok(
            "B738::ZBO
A20N :  : xpn",
        );
        assert_eq!(config.mappings["B738"].path, None);
        assert_eq!(config.mappings["B738"].format, "zbo");
        assert_eq!(config.mappings["A20N"].path, None);
        assert_eq!(config.mappings["A20N"].format, "xpn");
        // A path that is only separators is still an error, not the default directory.
        assert_eq!(error_lines("B738:/:zbo"), vec![1]);
        assert_eq!(error_lines("B738:./:zbo"), vec![1]);
    }

    #[test]
    fn empty_id_means_unset() {
        assert_eq!(parse_ok("simbrief_id:").simbrief_id, None);
        assert_eq!(parse_ok("simbrief_id:  ").simbrief_id, None);
    }

    #[test]
    fn format_is_lowercased_and_icao_uppercased() {
        let config = parse_ok("a20n:ToLiSS/A321:XPE");
        assert_eq!(config.mappings["A20N"].format, "xpe");
    }

    #[test]
    fn missing_id_is_not_an_error() {
        let config = parse_ok("B752:Boeing/757/co-routes");
        assert_eq!(config.simbrief_id, None);
    }

    #[test]
    fn invalid_ids() {
        assert_eq!(error_lines("simbrief_id:abc"), vec![1]);
        assert_eq!(error_lines("simbrief_id:0"), vec![1]);
        assert_eq!(error_lines("simbrief_id:-5"), vec![1]);
        assert_eq!(error_lines("simbrief_id:1:2"), vec![1]);
        assert_eq!(error_lines("simbrief_id:1\nsimbrief_id:2"), vec![2]);
    }

    #[test]
    fn bad_field_counts() {
        assert_eq!(error_lines("B752"), vec![1]);
        assert_eq!(error_lines("B752:a:b:c"), vec![1]);
        assert_eq!(error_lines(":Boeing/757"), vec![1]);
    }

    #[test]
    fn bad_formats() {
        assert_eq!(error_lines("B752:Boeing/757:"), vec![1]);
        assert_eq!(error_lines("B752:Boeing/757:v-5"), vec![1]);
    }

    #[test]
    fn bad_icao() {
        assert_eq!(error_lines("BOEING:Boeing/757"), vec![1]);
        assert_eq!(error_lines("B7 2:Boeing/757"), vec![1]);
    }

    #[test]
    fn duplicates_report_both_lines() {
        let (config, errors) = Config::parse("B752:a\nb752:b");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 2);
        assert!(errors[0].message.contains("line 1"));
        assert_eq!(config.mappings["B752"].path, Some(PathBuf::from("a")));
    }

    #[test]
    fn valid_lines_survive_invalid_ones() {
        let (config, errors) = Config::parse("simbrief_id:7\nB752:../x\nA320:ToLiSS");
        assert_eq!(errors.len(), 1);
        assert_eq!(config.simbrief_id, Some(7));
        assert!(config.mappings.contains_key("A320"));
    }

    #[test]
    fn path_validation() {
        let ok = PathBuf::from("Boeing").join("757").join("co-routes");
        assert_eq!(
            validate_aircraft_path("Boeing/757/co-routes"),
            Ok(ok.clone())
        );
        assert_eq!(
            validate_aircraft_path("Boeing\\757\\co-routes"),
            Ok(ok.clone())
        );
        assert_eq!(validate_aircraft_path("./Boeing//757/co-routes/"), Ok(ok));
        assert_eq!(
            validate_aircraft_path("Laminar Research/Boeing 737-800"),
            Ok(PathBuf::from("Laminar Research").join("Boeing 737-800"))
        );
        assert!(validate_aircraft_path("../Output").is_err());
        assert!(validate_aircraft_path("Boeing/../../etc").is_err());
        assert!(validate_aircraft_path("/etc").is_err());
        assert!(validate_aircraft_path("\\Windows").is_err());
        assert!(validate_aircraft_path("./").is_err());
        assert!(validate_aircraft_path("").is_err());
    }

    #[test]
    fn drive_letters_cannot_be_written() {
        // `C:\Aircraft` splits into `C` and `\Aircraft`, which is not a valid format.
        assert_eq!(error_lines("B752:C:\\Aircraft"), vec![1]);
    }

    #[test]
    fn load_or_create_writes_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs").join("SimBriefDownloader.txt");
        let (config, errors) = Config::load_or_create(&path).unwrap();
        assert!(errors.is_empty());
        assert_eq!(config, Config::parse(DEFAULT_CONFIG).0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);

        std::fs::write(&path, "simbrief_id:42").unwrap();
        let (config, errors) = Config::load_or_create(&path).unwrap();
        assert!(errors.is_empty());
        assert_eq!(config.simbrief_id, Some(42));
    }
}
