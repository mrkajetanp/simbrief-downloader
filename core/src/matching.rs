//! Check that the aircraft loaded in the sim is the one the flight plan was made for.

use crate::simbrief::Ofp;
use crate::Error;

/// Turns the raw `sim/aircraft/view/acf_ICAO` byte array into a string: cut at the first
/// NUL, decode lossily, trim whitespace.
pub fn normalize_acf_icao(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Passes only if the loaded aircraft's ICAO type equals the OFP's, ignoring case and
/// surrounding whitespace. Returns the uppercase ICAO type on success.
pub fn check_match(loaded_icao: &str, ofp: &Ofp) -> Result<String, Error> {
    let loaded = loaded_icao.trim();
    let planned = ofp.aircraft_icao.trim();
    if planned.is_empty() {
        return Err(Error::OfpNoIcao);
    }
    if loaded.is_empty() {
        return Err(Error::LoadedAircraftNoIcao);
    }
    if !loaded.eq_ignore_ascii_case(planned) {
        return Err(Error::AircraftMismatch {
            planned_icao: planned.to_ascii_uppercase(),
            planned_name: ofp.aircraft_name.clone(),
            loaded_icao: loaded.to_ascii_uppercase(),
        });
    }
    Ok(planned.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ofp(icao: &str) -> Ofp {
        Ofp {
            request_id: None,
            aircraft_icao: icao.into(),
            aircraft_name: "FlightFactor 757".into(),
            origin: "KJFK".into(),
            destination: "EGLL".into(),
            downloads_directory: String::new(),
            downloads: BTreeMap::new(),
        }
    }

    #[test]
    fn normalizes_dataref_bytes() {
        let mut raw = [0u8; 40];
        raw[..4].copy_from_slice(b"B752");
        assert_eq!(normalize_acf_icao(&raw), "B752");
        assert_eq!(normalize_acf_icao(b" b752  \0garbage"), "b752");
        assert_eq!(normalize_acf_icao(b"B752"), "B752");
        assert_eq!(normalize_acf_icao(&[0u8; 40]), "");
        assert_eq!(normalize_acf_icao(b""), "");
    }

    #[test]
    fn exact_and_case_insensitive_matches_pass() {
        assert_eq!(check_match("B752", &ofp("B752")), Ok("B752".into()));
        assert_eq!(check_match("b752", &ofp("B752")), Ok("B752".into()));
        assert_eq!(check_match(" B752 ", &ofp("b752")), Ok("B752".into()));
    }

    #[test]
    fn mismatch_fails() {
        assert_eq!(
            check_match("A20N", &ofp("B752")),
            Err(Error::AircraftMismatch {
                planned_icao: "B752".into(),
                planned_name: "FlightFactor 757".into(),
                loaded_icao: "A20N".into(),
            })
        );
        // Prefixes are not matches.
        assert!(check_match("B75", &ofp("B752")).is_err());
    }

    #[test]
    fn empty_values_fail() {
        assert_eq!(
            check_match("", &ofp("B752")),
            Err(Error::LoadedAircraftNoIcao)
        );
        assert_eq!(
            check_match("  ", &ofp("B752")),
            Err(Error::LoadedAircraftNoIcao)
        );
        assert_eq!(check_match("B752", &ofp("")), Err(Error::OfpNoIcao));
        assert_eq!(check_match("", &ofp("")), Err(Error::OfpNoIcao));
    }

    #[test]
    fn mismatch_message_names_both_aircraft() {
        let message = check_match("A20N", &ofp("B752")).unwrap_err().to_string();
        assert_eq!(
            message,
            "Flight plan is for B752 (FlightFactor 757), but the loaded aircraft is A20N; nothing downloaded"
        );
    }
}
