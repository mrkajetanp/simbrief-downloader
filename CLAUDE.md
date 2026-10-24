# SimBrief Downloader — X-Plane 12 plugin (Rust)

An X-Plane 12 plugin that fetches the latest flight plan for a SimBrief pilot ID and
puts it in the directory the current aircraft expects. Most aircraft read from
`X-Plane 12/Output/FMS plans/`, and some add-ons have their own directory under
`Aircraft/` (for example the FlightFactor 757 reads from
`Aircraft/Boeing/757/co-routes/`). Users map aircraft ICAO codes to directories in a
config file.

## Goals

1. Fetch the most recent OFP for a configured SimBrief pilot ID (numeric user ID).
2. Read the aircraft ICAO type from the OFP (e.g. `B752`) and compare it with the ICAO
   type of the aircraft loaded in the sim. **If they differ, download nothing and show
   an error** (see Aircraft match check).
3. Look the ICAO code up in the `icao:path[:format]` mappings. Use the matched path,
   which is relative to `<X-Plane>/Aircraft/`, and its SimBrief download format. If
   nothing matches, use `<X-Plane>/Output/FMS plans/` and the X-Plane `.fms` format.
4. Download the flight plan file and write it there. The download must never block or
   crash the sim.

Downloads happen only when the user asks for one (menu item or command). **There
is no automatic download.** Don't trigger downloads on aircraft load, on a timer or
on any other event.

## Workspace layout

A Cargo workspace with two crates, so the core logic can be tested without X-Plane:

```
Cargo.toml                  # [workspace] members = ["core", "plugin"]
core/                       # simbrief-core: pure Rust, no XPLM dependency
  src/lib.rs                # Error type, download_flight_plan() flow
  src/config.rs             # config file parsing + validation
  src/simbrief.rs           # API response parsing, HttpClient trait, ureq client
  src/routing.rs            # ICAO -> target directory resolution, file naming
  src/matching.rs           # loaded aircraft vs OFP ICAO check
  src/write.rs              # atomic file write
  tests/fixtures/           # sanitized SimBrief responses + a real FF757 .flp
plugin/                     # simbrief-plugin: cdylib, the only crate that touches XPLM
  build.rs                  # macOS: -undefined dynamic_lookup
  src/lib.rs                # XPlugin* entry points, plugin state, actions
  src/ffi.rs                # hand-written XPLM declarations
  src/xp.rs                 # safe wrappers: log, paths, dataref, menu, command, flight loop
  src/message.rs            # self-hiding on-screen notification window
  src/worker.rs             # background download thread + result channel
tools/
  build-dist.sh             # build + lay out dist/SimBriefDownloader/lin_x64/*.xpl
  zigcc, zigar              # Zig wrappers for cross-compiling to x86_64 Linux
```

`core` must not touch XPLM. A test binary that links XPLM symbols fails to link outside
the sim, so all logic that needs testing goes in `core`.

## Dependencies

- **No XPLM binding crate.** The ~25 XPLM functions used are declared by hand in
  `plugin/src/ffi.rs`, checked against the X-Plane 12 SDK headers. This avoids
  bindgen/libclang and needing the SDK at build time, which matters for
  cross-compiling. When adding a call, copy its signature from the SDK header (the
  user has a copy at `~/dev/XTLua/SDK/CHeaders`) and wrap it in `xp.rs`.
  - Linux: XPLM symbols stay undefined and resolve when X-Plane loads the plugin.
  - Windows: `#[link(name = "XPLM_64", kind = "raw-dylib")]`, so no import library.
  - macOS: `build.rs` passes `-undefined dynamic_lookup`.
- `ureq` for HTTP with the `rustls` TLS backend, not OpenSSL. It is blocking, which is
  fine on a worker thread, and it avoids a tokio runtime and system OpenSSL linking
  problems when cross-compiling.
- `serde` and `serde_json` for the API response.
- `thiserror` in `core` for typed errors.

Keep the dependency tree small. Do not add an async runtime.

## X-Plane plugin rules (important)

- **Call XPLM APIs only from the main sim thread.** That covers logging, menus,
  datarefs and paths. The HTTP request and file write run on a `std::thread` worker.
  The worker sends a result enum over `std::sync::mpsc`, and a flight loop callback
  on the main thread polls the channel with `try_recv` and does the logging and
  notifications.
- **Never block the sim thread.** No network calls in `start`/`enable` or in menu
  handlers. Handlers only spawn the worker. Allow one download at a time: if a request
  is already in flight, ignore new ones and log that.
- **No panics across FFI.** Wrap every callback body, including menu, command and
  flight loop handlers, in `std::panic::catch_unwind` and log the panic. Do not set
  `panic = "abort"`, because that would take X-Plane down with it. Use `Result`
  everywhere in `core`; no `unwrap()`/`expect()` outside tests.
- **Native paths:** call `XPLMEnableFeature("XPLM_USE_NATIVE_PATHS", 1)` in `start`.
  Get the X-Plane root from `XPLMGetSystemPath` and build every path with
  `std::path::PathBuf`. Never concatenate path strings, because separators differ
  between Windows, macOS and Linux.
- **Logging:** use `XPLMDebugString` (writes to `Log.txt`) and prefix every line with
  `[SimBrief Downloader] `. Log the resolved target path on every download.
- **Shutdown:** in `disable`/`stop`, unregister the flight loop and destroy the menu.
  Do not join a worker thread that may be stuck on the network. Detach it and drop
  its results.
- **State:** a `thread_local!` `RefCell<Option<Plugin>>`, created in `XPluginEnable`
  and taken out and dropped in `XPluginDisable`. Menu, commands, flight loop and window
  are RAII wrappers whose `Drop` unregisters them. The flight loop runs every 0.25 s to
  poll the worker and hide expired messages.
- Plugin signature: `com.github.kajetan.simbriefdownloader`. Name: `SimBrief Downloader`.

## User interface

- Menu under **Plugins → SimBrief Downloader**:
  - `Download flight plan`: the download button. It fetches the latest OFP, runs the
    aircraft match check, and saves the flight plan only if the check passes.
  - `Reload config`: re-reads the config file.
- Custom commands, so users can bind them to keys or joystick buttons:
  - `simbrief_downloader/download`
  - `simbrief_downloader/reload_config`
- Feedback: log every result. Also show a short on-screen message, using an
  `XPLMCreateWindowEx` message window that disappears after about 5 s or
  `XPLMSpeakString`, e.g. `Saved KJFKEGLL.fms to Boeing/757/co-routes` or
  `SimBrief error: <message>`.
- An aircraft mismatch is an error shown on screen, e.g.
  `Flight plan is for B752 (FlightFactor 757), but the loaded aircraft is A20N — nothing downloaded`.
  Use `aircraft.name` from the OFP in the message.

## Aircraft match check

Run this check before every download. No flight plan file is
downloaded or written unless it passes. Fetching the OFP JSON is allowed, because
the check needs it.

- **Loaded aircraft ICAO:** dataref `sim/aircraft/view/acf_ICAO`, a byte array. Read
  it with `XPLMGetDatab` **on the main thread** when the download is triggered and
  pass the string to the worker. Trim it at the first NUL byte and trim whitespace.
- **OFP ICAO:** `aircraft.icaocode`.
- Compare the two case-insensitively, after trimming. An exact match passes. Anything
  else fails, including:
  - An empty `acf_ICAO`. Some aircraft don't set it. The error says the loaded
    aircraft reports no ICAO type.
  - A missing or empty `aircraft.icaocode` in the OFP.
- Order of work on the worker: fetch the JSON, check `fetch.status`, run the match
  check, resolve the target directory, then download the file. Stop at the first
  failure.
- The match uses the same ICAO that selects the config mapping, so the directory is
  always chosen for the aircraft that is actually loaded.

Do not build an in-sim text entry widget for the pilot ID in v1. It goes in the
config file.

## Configuration

File: `<X-Plane>/Output/preferences/SimBriefDownloader.txt`. It is kept outside the
plugin folder so that updating the plugin does not overwrite it. If the file is
missing, create it with commented examples on first start.

Format: one entry per line. Blank lines and lines that start with `#` are ignored.
Split each line on `:` and trim whitespace around every field.

```
# SimBrief pilot ID (numeric, from https://dispatch.simbrief.com/account)
simbrief_id:123456

# ICAO:path[:format]
#   path   - directory relative to <X-Plane>/Aircraft/
#   format - optional SimBrief fms_downloads key (default: xpe = X-Plane 11/12 .fms)
#   FlightFactor 757 V2 reads its own .flp route format from co-routes:
B752:Boeing/757/co-routes:vm5
B753:Boeing/757/co-routes:vm5
```

`vm5` is the FF757 V2 key, confirmed against a real response. Don't confuse it with
`vmx`, which is the FF 767/777 format. Both download as `.flp`.

Rules:

- `simbrief_id:<n>` is a reserved key and must parse as a positive integer.
- Every other line is `ICAO:path` or `ICAO:path:format`, i.e. 2 or 3 fields. Any
  other field count is an error. As a result, paths cannot contain `:`, which
  Windows does not allow anyway.
- The ICAO key is an aircraft type designator, matched case-insensitively. Store
  keys uppercased.
- `format` is a key under `fms_downloads` in the SimBrief response (e.g. `xpe`). It
  is stored lowercased and must be non-empty ASCII alphanumeric. If it is missing or
  empty, it defaults to `xpe`. Unmapped aircraft also use `xpe`. Whether the key
  exists is checked against the API response at download time, not when the config
  is parsed.
- Mapping paths are **always relative to `<X-Plane>/Aircraft/`**. Reject absolute
  paths, paths with drive prefixes, and any path with a `..` component, so that a path
  cannot point outside `Aircraft/`. Accept both `/` and `\` as separators in the file.
- An ICAO code that appears twice is a config error: report the line numbers.
- Report config errors as a list with line numbers and keep the valid entries. A bad
  line must not disable the plugin.
- If a mapped directory does not exist, **do not create it**. Report an error naming
  the path. A missing directory usually means the aircraft was moved or renamed, and
  silently creating it would hide that. The default `Output/FMS plans` directory may be
  created if it is missing.

## SimBrief API

- Endpoint: `https://www.simbrief.com/api/xml.fetcher.php?userid=<id>&json=1`. This
  returns the latest OFP for that user. No API key is needed for this endpoint.
- Fields used (all confirmed against a real response):
  - `fetch.status`: `"Success"` or an error message, e.g. when the user has no OFP.
    A non-success status is an error and its text is shown to the user.
  - `aircraft.icaocode`: the ICAO type used for routing, e.g. `E55P`.
  - `origin.icao_code` and `destination.icao_code`: used for the file name.
  - `fms_downloads.directory`: the base URL for downloads,
    `https://www.simbrief.com/ofp/flightplans/`.
  - `fms_downloads.<format>`: an object `{ "name": "<label>", "link": "<link>" }`.
    The `name` is a human-readable label such as `"FlightFactor 757 V2"`. Show it in
    error messages and logs.
- Deserialize only the fields above, into structs with `#[serde(default)]` and
  optional fields where that makes sense. Deserialize `fms_downloads` as a map
  (`HashMap<String, serde_json::Value>`), because the format key is chosen at
  runtime and `directory` sits next to the format entries. The response is large and
  its shape changes, so do not model all of it.
- **Do not parse meaning out of `link`.** Real examples show it is not a reliable
  `{ORIG}{DEST}_{KEY}_{ts}.{ext}` pattern:
  - `xpe` → `SLLPSABE_XPE_1792870617.fms`
  - `vm5` → `SLLPSABE_VMX_1792870617.flp--VM5`: it reuses another format's file,
    with a `--KEY` suffix.
  - `zbo` → `xml/SLLPSABE_XML_1792870617.xml--ZBO`: it includes a subdirectory.
  - `a3e` → `SLLPSABE_PDF_1792870617.pdf--A3E`
- **Download URL:** `directory + link`, used verbatim and not re-encoded. That URL
  returns `302` to
  `https://www.simbrief.com/system/briefing.fmsdl.php?formatget=flightplans/<link>`,
  which serves the file as `200 application/octet-stream` with
  `Content-Disposition: attachment; filename="SLLPSABE.flp"`. The HTTP client must
  follow redirects (ureq does by default). Checked for `xpe` and `vm5`.
- A redirect target or response with `Content-Type: text/html` means an error page,
  not a flight plan.
- If the configured format key is missing from `fms_downloads`, report an error that
  lists the keys that are available. That way the user can fix the config without
  looking at the raw response.
- Set a `User-Agent` such as `SimBriefDownloader/<version>`, a connect timeout of
  about 10 s and a total timeout of about 30 s.
- If the downloaded file is empty or is an HTML error page, treat it as an error.

## Output file

- File name: `{ORIGIN}{DEST}{ext}` (e.g. `KJFKEGLL.fms`), with `ORIGIN` and `DEST`
  uppercase, overwriting any existing file of that name. Most FMSs list plans by this
  name. Get `ext` in this order:
  1. The extension of the `Content-Disposition` filename (the server sends
     `SLLPSABE.flp` for the FF757).
  2. Otherwise, the `link`: drop everything from the first `--`, take the last `/`
     segment, and use its extension (`xml/X_XML_1.xml--ZBO` → `.xml`).
  3. If neither gives an extension, report an error.

  Keep the extension's case as sent; some add-ons expect e.g. `.RTE` or `.FLTPLAN`.
  Never use the server's filename as the output path as is. Only take its extension,
  and validate that it has no path separators.
- Write atomically: write to a temp file in the target directory (`.{name}.tmp`),
  `fsync`, then `rename` over the final name. If anything fails, remove the temp file.

## Open questions

- Is `acf_ICAO` correct for the add-ons that matter, e.g. does the FF757 report
  `B752`/`B753`? If an aircraft reports an unexpected code, ask the user before
  adding any alias mechanism. Strict equality is the specified behaviour.

## Build & packaging

- `plugin/Cargo.toml`: `crate-type = ["cdylib"]`.
- Output layout. X-Plane 12 expects:
  ```
  X-Plane 12/Resources/plugins/SimBriefDownloader/
    lin_x64/SimBriefDownloader.xpl
    win_x64/SimBriefDownloader.xpl
    mac_x64/SimBriefDownloader.xpl   # universal (x86_64 + arm64) binary via lipo
  ```
- **Linux (done):** `tools/build-dist.sh` builds `x86_64-unknown-linux-gnu` and copies
  `libsimbrief_plugin.so` to `dist/SimBriefDownloader/lin_x64/SimBriefDownloader.xpl`.
  - The user's dev machine is **aarch64 Linux**, and X-Plane on Linux is x86_64 only.
    So the script cross-compiles through Zig (`tools/zigcc`, target glibc 2.28),
    for both the final link and `ring`'s C code.
  - Zig is not on PATH there. Use
    `ZIG="$HOME/.cache/re-tools/zigenv/bin/python -m ziglang" tools/build-dist.sh`.
  - The Rust target needed is `rustup target add x86_64-unknown-linux-gnu`.
  - Check the result with `readelf -d` (only libc, libpthread, libdl and ld-linux
    needed), `objdump -T` (GLIBC ≤ 2.28) and `nm -D --defined-only` (only the five
    `XPlugin*` symbols).
- **Windows / macOS: not set up yet.** The FFI is prepared (raw-dylib, dynamic_lookup),
  but no build has been made or tested. Windows would be `x86_64-pc-windows-gnu` or
  MSVC. macOS needs `x86_64-apple-darwin` + `aarch64-apple-darwin` combined with
  `lipo`, which requires macOS or osxcross. Ask the user before adding either.

## Testing

- `cargo test -p simbrief-core` must pass without X-Plane installed. Cover:
  - Config parsing: comments, blank lines, whitespace, case-insensitive ICAO,
    duplicate keys, a missing or invalid `simbrief_id`, 2-field vs 3-field lines,
    an empty or invalid format field, too many `:` fields.
  - Path validation: rejects `..`, absolute paths, `C:\...`, and accepts both
    separator styles.
  - Routing: a mapped ICAO resolves to `Aircraft/<path>`, an unmapped ICAO resolves
    to `Output/FMS plans`, a mapped directory that does not exist is an error.
  - Response parsing from fixtures: success, error status, a format key that the
    response doesn't have (the error lists the available keys).
  - File naming: the extension comes from `Content-Disposition` when it is present,
    else from the link. Cover the link shapes `X_XPE_1.fms`, `X_VMX_1.flp--VM5`,
    `xml/X_XML_1.xml--ZBO`, a link with no extension (error), and a malicious
    `Content-Disposition` filename such as `../../evil.fms` (only the extension is
    used).
  - Match check: an exact match, a case difference, NUL-padded or whitespace-padded
    `acf_ICAO`, an empty `acf_ICAO`, a missing OFP ICAO, and a mismatch. A mismatch
    must make no file-download call: assert this with the fake HTTP trait.
  - Atomic write: the final file exists, no temp file is left behind, and an existing
    file is overwritten.
- Use `tempfile` for a fake X-Plane root in tests. Abstract the HTTP fetch behind a
  small trait so the download flow can be tested with canned responses, with no
  network in tests.
- X-Plane can't run on the aarch64 dev machine. During development the native aarch64
  build was exercised end-to-end with a throwaway C host that implements the XPLM
  functions, `dlopen`s the plugin, and drives the menu, commands, flight loop and
  window draw. If plugin-side behaviour changes substantially, rebuild a similar
  harness (link it with `-rdynamic` so the plugin resolves XPLM symbols from it), and
  keep live requests to a minimum.
- Manual check in the sim: install to `Resources/plugins`, run Download, confirm the
  `Log.txt` output and the file location for a mapped ICAO and an unmapped ICAO.
  Load a non-matching aircraft and confirm the error appears and no file is written.

## Development against the live API

- SimBrief pilot ID for development: the user's own account (not stored in the repo). Use it
  for real requests.
- **Keep live requests to a minimum; don't spam SimBrief's servers.** Fetch the OFP
  JSON once and save it as a fixture in `core/tests/fixtures/`. Develop and test
  against the saved file. Make a live request only to check something new, and a
  few per session at most. Never make live requests from tests, loops or CI.
- Before committing a fixture, strip personal data from it (the user ID, name,
  and any `params`/`api` fields that contain identifiers). Trim it to the fields
  the code reads plus a few `fms_downloads` entries (`xpe`, `vm5`, `vmx`, `zbo`).

## Commands

```
tools/build-dist.sh                         # build the X-Plane plugin into dist/ (see Build)
cargo build -p simbrief-plugin              # native build, for compile checks / harness
cargo test -p simbrief-core                 # run unit tests (no X-Plane needed)
cargo clippy --workspace --all-targets -- -D warnings   # lint; keep it clean
cargo fmt --all                             # format
```

## Conventions

- Rust 2021 edition. The code must build on stable (the user's default is nightly).
- `unsafe` only in `plugin`, kept in small functions with a `// SAFETY:` comment.
- Error messages shown to the user must be short and actionable. Name the path or the
  config line involved.
