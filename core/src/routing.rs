//! Where a flight plan goes and what it is called.

use std::path::{Component, Path, PathBuf};

use crate::config::{Config, DEFAULT_FORMAT};
use crate::simbrief::Ofp;
use crate::Error;

/// The directory and SimBrief format chosen for an aircraft type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub dir: PathBuf,
    /// SimBrief `fms_downloads` key.
    pub format: String,
    /// `dir` relative to the X-Plane root with `/` separators, for messages.
    pub display_dir: String,
}

/// Resolves the target for an ICAO type. A mapped directory under `Aircraft/` must already
/// exist; the default `Output/FMS plans` directory (used by unmapped types and by mappings
/// with an empty path) is created if needed.
pub fn resolve_target(xplane_root: &Path, config: &Config, icao: &str) -> Result<Target, Error> {
    let mapping = config.mappings.get(&icao.to_ascii_uppercase());
    let format = mapping.map_or(DEFAULT_FORMAT, |m| &m.format).to_string();
    let (relative, create) = match mapping.and_then(|m| m.path.as_ref()) {
        Some(path) => (Path::new("Aircraft").join(path), false),
        None => (default_dir(), true),
    };
    let dir = xplane_root.join(&relative);
    let display_dir = display_path(&relative);

    if create {
        std::fs::create_dir_all(&dir).map_err(|e| Error::Write {
            path: display_dir.clone(),
            message: e.to_string(),
        })?;
    } else if !dir.is_dir() {
        return Err(Error::MissingDirectory(display_dir));
    }

    Ok(Target {
        dir,
        format,
        display_dir,
    })
}

/// `Output/FMS plans`, relative to the X-Plane root.
pub fn default_dir() -> PathBuf {
    Path::new("Output").join("FMS plans")
}

pub fn display_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Formats whose add-on only reads one fixed file name, by SimBrief format key.
const FIXED_FILE_NAMES: &[(&str, &str)] = &[
    // The Zibo 737 only loads its SimBrief XML from `Output/FMS plans/b738x.xml`.
    ("zbo", "b738x.xml"),
];

/// The fixed file name a format must be saved as, if it has one.
pub fn fixed_file_name(format: &str) -> Option<&'static str> {
    FIXED_FILE_NAMES
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(format))
        .map(|(_, name)| *name)
}

/// Output file name. Formats in [`FIXED_FILE_NAMES`] always get their fixed name; all
/// others are `{ORIGIN}{DEST}.{ext}`. The extension comes from the server's
/// `Content-Disposition` filename if it has a usable one, otherwise from the link (with any
/// `--KEY` suffix and directory removed). Only the extension of server-provided names is
/// ever used, so they cannot influence the directory.
pub fn output_file_name(
    ofp: &Ofp,
    format: &str,
    content_disposition: Option<&str>,
    link: &str,
) -> Result<String, Error> {
    if let Some(name) = fixed_file_name(format) {
        return Ok(name.to_string());
    }
    let origin = airport_code(&ofp.origin, "origin")?;
    let destination = airport_code(&ofp.destination, "destination")?;
    let extension = content_disposition
        .and_then(disposition_filename)
        .and_then(|name| extension_of(&name))
        .or_else(|| link_extension(link))
        .ok_or_else(|| {
            Error::InvalidResponse(format!("cannot tell the file type of download '{link}'"))
        })?;
    Ok(format!("{origin}{destination}.{extension}"))
}

fn airport_code(code: &str, which: &str) -> Result<String, Error> {
    if code.is_empty() || code.len() > 8 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::InvalidResponse(format!(
            "invalid {which} airport code '{code}'"
        )));
    }
    Ok(code.to_ascii_uppercase())
}

/// `filename` parameter of a Content-Disposition header, without quotes.
fn disposition_filename(header: &str) -> Option<String> {
    header.split(';').find_map(|param| {
        let (key, value) = param.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("filename") {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        Some(value.to_string())
    })
}

fn link_extension(link: &str) -> Option<String> {
    let without_suffix = link.split("--").next().unwrap_or(link);
    extension_of(without_suffix)
}

/// Extension of the last path segment, case preserved. Must be 1-16 ASCII letters/digits.
fn extension_of(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (_, extension) = base.rsplit_once('.')?;
    let valid =
        (1..=16).contains(&extension.len()) && extension.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then(|| extension.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ofp() -> Ofp {
        Ofp {
            request_id: None,
            aircraft_icao: "B752".into(),
            aircraft_name: String::new(),
            origin: "kjfk".into(),
            destination: "EGLL".into(),
            downloads_directory: String::new(),
            downloads: BTreeMap::new(),
        }
    }

    fn name(disposition: Option<&str>, link: &str) -> Result<String, Error> {
        output_file_name(&ofp(), "xpe", disposition, link)
    }

    #[test]
    fn zibo_format_is_always_b738x_xml() {
        let disposition = Some("attachment; filename=\"SLLPSABE.xml\"");
        let link = "xml/X_XML_1.xml--ZBO";
        assert_eq!(
            output_file_name(&ofp(), "zbo", disposition, link).unwrap(),
            "b738x.xml"
        );
        assert_eq!(
            output_file_name(&ofp(), "ZBO", None, link).unwrap(),
            "b738x.xml"
        );
        // Other formats with the same file type keep the airport naming.
        assert_eq!(
            output_file_name(&ofp(), "xml", disposition, link).unwrap(),
            "KJFKEGLL.xml"
        );
    }

    #[test]
    fn extension_from_link_shapes() {
        assert_eq!(name(None, "X_XPE_1.fms").unwrap(), "KJFKEGLL.fms");
        assert_eq!(name(None, "X_VMX_1.flp--VM5").unwrap(), "KJFKEGLL.flp");
        assert_eq!(name(None, "xml/X_XML_1.xml--ZBO").unwrap(), "KJFKEGLL.xml");
        assert_eq!(name(None, "X_IFY_1.FLTPLAN").unwrap(), "KJFKEGLL.FLTPLAN");
        assert!(matches!(
            name(None, "X_XPE_1"),
            Err(Error::InvalidResponse(_))
        ));
        assert!(matches!(
            name(None, "dir.v2/X_XPE_1"),
            Err(Error::InvalidResponse(_))
        ));
    }

    #[test]
    fn extension_from_content_disposition() {
        assert_eq!(
            name(
                Some("attachment; filename=\"SLLPSABE.flp\""),
                "X_PDF_1.pdf--A3E"
            )
            .unwrap(),
            "KJFKEGLL.flp"
        );
        assert_eq!(
            name(Some("attachment; FileName=plan.RTE"), "X.fms").unwrap(),
            "KJFKEGLL.RTE"
        );
    }

    #[test]
    fn hostile_content_disposition_only_contributes_extension() {
        assert_eq!(
            name(Some("attachment; filename=\"../../evil.fms\""), "X.flp").unwrap(),
            "KJFKEGLL.fms"
        );
        assert_eq!(
            name(Some("attachment; filename=\"..\\..\\evil.fms\""), "X.flp").unwrap(),
            "KJFKEGLL.fms"
        );
        // An unusable extension falls back to the link.
        assert_eq!(
            name(Some("attachment; filename=\"evil.f/ms\""), "X.flp").unwrap(),
            "KJFKEGLL.flp"
        );
        assert_eq!(name(Some("attachment"), "X.flp").unwrap(), "KJFKEGLL.flp");
    }

    #[test]
    fn invalid_airports_are_rejected() {
        let mut bad = ofp();
        bad.origin = "../x".into();
        assert!(output_file_name(&bad, "xpe", None, "X.fms").is_err());
        bad.origin = String::new();
        assert!(output_file_name(&bad, "xpe", None, "X.fms").is_err());
    }

    fn config(text: &str) -> Config {
        let (config, errors) = Config::parse(text);
        assert!(errors.is_empty());
        config
    }

    #[test]
    fn mapped_icao_resolves_under_aircraft() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("Aircraft")
            .join("Boeing")
            .join("757")
            .join("co-routes");
        std::fs::create_dir_all(&dir).unwrap();
        let target = resolve_target(
            root.path(),
            &config("B752:Boeing/757/co-routes:vm5"),
            "b752",
        )
        .unwrap();
        assert_eq!(
            target,
            Target {
                dir,
                format: "vm5".into(),
                display_dir: "Aircraft/Boeing/757/co-routes".into(),
            }
        );
    }

    #[test]
    fn missing_mapped_directory_is_an_error_and_not_created() {
        let root = tempfile::tempdir().unwrap();
        let result = resolve_target(root.path(), &config("B752:Boeing/757/co-routes"), "B752");
        assert_eq!(
            result,
            Err(Error::MissingDirectory(
                "Aircraft/Boeing/757/co-routes".into()
            ))
        );
        assert!(!root.path().join("Aircraft").exists());
    }

    #[test]
    fn unmapped_icao_uses_fms_plans_and_creates_it() {
        let root = tempfile::tempdir().unwrap();
        let target = resolve_target(root.path(), &config("B752:Boeing/757"), "A20N").unwrap();
        assert_eq!(target.dir, root.path().join("Output").join("FMS plans"));
        assert_eq!(target.format, "xpe");
        assert_eq!(target.display_dir, "Output/FMS plans");
        assert!(target.dir.is_dir());
    }

    #[test]
    fn empty_mapped_path_uses_fms_plans_with_mapped_format() {
        let root = tempfile::tempdir().unwrap();
        let target = resolve_target(root.path(), &config("B738::zbo"), "b738").unwrap();
        assert_eq!(target.dir, root.path().join("Output").join("FMS plans"));
        assert_eq!(target.format, "zbo");
        assert_eq!(target.display_dir, "Output/FMS plans");
        assert!(target.dir.is_dir());
    }
}
