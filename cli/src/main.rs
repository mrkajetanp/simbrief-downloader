//! `simbrief-dev`: exercises the plugin's download logic from the command line, without
//! X-Plane. `fetch` runs exactly the flow the plugin's Download button runs, with the
//! "loaded aircraft" given on the command line and a fake X-Plane root on disk.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use simbrief_core::config::Config;
use simbrief_core::simbrief::{ofp_url, HttpClient, Ofp, UreqClient};
use simbrief_core::{config_path, download_flight_plan, routing, DownloadRequest};

const USER_AGENT: &str = concat!("SimBriefDownloader-dev/", env!("CARGO_PKG_VERSION"));
const DEFAULT_ROOT: &str = "target/xplane-sandbox";

const USAGE: &str = "\
simbrief-dev: test SimBrief Downloader without X-Plane

USAGE:
    simbrief-dev fetch --type <ICAO> [--format <KEY>] [--id <N>] [--root <DIR>]
        Download the latest flight plan exactly as the plugin does, pretending the
        aircraft <ICAO> is loaded. Refuses if the flight plan is for another type.
        --format overrides the format from the config (e.g. xpe, vm5).

    simbrief-dev info [--id <N>] [--root <DIR>]
        Show the latest flight plan's aircraft, route and available download formats.

    simbrief-dev config [--root <DIR>]
        Check the config file and show where each aircraft type's plans would go.
        No network access.

OPTIONS:
    --root <DIR>  Fake X-Plane root (default: target/xplane-sandbox). The config is
                  <DIR>/Output/preferences/SimBriefDownloader.txt, created if missing.
                  Mapped directories under <DIR>/Aircraft must exist; create them with
                  mkdir -p to test a mapping.
    --id <N>      SimBrief pilot ID; default: simbrief_id from the config, then the
                  SIMBRIEF_ID environment variable.

fetch and info each make live requests to SimBrief; please don't run them in a loop.";

#[derive(Default)]
struct Args {
    command: String,
    icao: Option<String>,
    format: Option<String>,
    id: Option<u64>,
    root: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let mut args = Args {
        command: raw.next().ok_or("missing command")?,
        ..Args::default()
    };
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--type" | "-t" => args.icao = Some(value()?),
            "--format" | "-f" => args.format = Some(value()?),
            "--id" => {
                let id = value()?;
                args.id = Some(id.parse().map_err(|_| format!("invalid --id '{id}'"))?);
            }
            "--root" => args.root = Some(PathBuf::from(value()?)),
            "-h" | "--help" => args.command = "help".into(),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let root = args
        .root
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT));
    let result = match args.command.as_str() {
        "fetch" => fetch(&args, &root),
        "info" => info(&args, &root),
        "config" => show_config(&root).map(|_| ()),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("error: unknown command '{other}'\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(root: &Path) -> Result<Config, String> {
    let path = config_path(root);
    let (config, errors) = Config::load_or_create(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    println!("config: {}", path.display());
    for error in &errors {
        println!("  config error, {error} (line ignored)");
    }
    Ok(config)
}

fn simbrief_id(args: &Args, config: &Config) -> Result<u64, String> {
    args.id
        .or(config.simbrief_id)
        .or_else(|| std::env::var("SIMBRIEF_ID").ok()?.trim().parse().ok())
        .ok_or_else(|| {
            "no SimBrief ID: pass --id, set simbrief_id in the config, or set SIMBRIEF_ID".into()
        })
}

fn fetch(args: &Args, root: &Path) -> Result<(), String> {
    let icao = args.icao.clone().ok_or("fetch needs --type <ICAO>")?;
    let config = load_config(root)?;
    let request = DownloadRequest {
        simbrief_id: simbrief_id(args, &config)?,
        xplane_root: root.to_path_buf(),
        config,
        loaded_icao: icao.clone(),
        format_override: args.format.clone(),
    };
    println!(
        "fetching latest flight plan for pilot ID {}, loaded aircraft {icao}...",
        request.simbrief_id
    );
    let client = UreqClient::new(USER_AGENT);
    match download_flight_plan(&client, &request) {
        Ok(saved) => {
            println!(
                "saved {} ({}, format '{}')\n   to {}",
                saved.file_name,
                saved.format_name,
                saved.format,
                saved.path.display()
            );
            Ok(())
        }
        Err(error) => {
            if let Some(details) = error.details() {
                println!("{details}");
            }
            Err(error.to_string())
        }
    }
}

fn info(args: &Args, root: &Path) -> Result<(), String> {
    let config = load_config(root)?;
    let id = simbrief_id(args, &config)?;
    let client = UreqClient::new(USER_AGENT);
    let response = client
        .get(&ofp_url(id))
        .map_err(|e| format!("network: {e}"))?;
    let ofp = Ofp::parse(&response.body).map_err(|e| e.to_string())?;

    println!("pilot ID:   {id}");
    println!(
        "request ID: {}",
        ofp.request_id.as_deref().unwrap_or("(none)")
    );
    println!("aircraft:   {} ({})", ofp.aircraft_icao, ofp.aircraft_name);
    println!("route:      {} -> {}", ofp.origin, ofp.destination);
    match routing::resolve_target(root, &config, &ofp.aircraft_icao) {
        Ok(target) => println!(
            "would save: {} as format '{}'",
            target.display_dir, target.format
        ),
        Err(error) => println!("would save: nowhere ({error})"),
    }
    println!("\nformats ({}):", ofp.downloads.len());
    for (key, download) in &ofp.downloads {
        println!("  {key:<4} {:<32} {}", download.name, download.link);
    }
    Ok(())
}

fn show_config(root: &Path) -> Result<Config, String> {
    let config = load_config(root)?;
    match config.simbrief_id {
        Some(id) => println!("simbrief_id: {id}"),
        None => println!("simbrief_id: (not set)"),
    }
    let mut mappings: Vec<_> = config.mappings.iter().collect();
    mappings.sort_by_key(|(_, mapping)| mapping.line);
    if mappings.is_empty() {
        println!("no aircraft mappings; every type goes to Output/FMS plans as xpe");
    }
    for (icao, mapping) in mappings {
        let status = match routing::resolve_target(root, &config, icao) {
            Ok(target) => format!("ok: {}", target.dir.display()),
            Err(error) => error.to_string(),
        };
        let dir = match &mapping.path {
            Some(path) => routing::display_path(&Path::new("Aircraft").join(path)),
            None => routing::display_path(&routing::default_dir()),
        };
        println!("  {icao:<4} -> {dir} [{}]  {status}", mapping.format);
    }
    Ok(config)
}
