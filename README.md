# SimBrief Downloader

An X-Plane 12 plugin that downloads your latest SimBrief flight plan and saves it where
the loaded aircraft looks for it.

Most aircraft get an X-Plane `.fms` file in `Output/FMS plans`. Add-ons with their own
route directory and format, like the FlightFactor 757's `.flp` routes in
`co-routes`, can be configured per aircraft type.

The plugin only downloads when you ask, and only if the aircraft loaded in the sim
matches the aircraft type in the flight plan. Otherwise it shows an error and saves
nothing.

## Install

Copy `SimBriefDownloader` (the folder containing `lin_x64/`) into
`X-Plane 12/Resources/plugins/`.

## Use

- **Plugins → SimBrief Downloader → Download flight plan** fetches your latest SimBrief
  flight plan and saves it. The result is shown at the top of the screen and written to
  `Log.txt`.
- **Reload config** re-reads the config file after you edit it.
- Both are also available as commands you can bind to a key or button:
  `simbrief_downloader/download` and `simbrief_downloader/reload_config`.

## Configure

The config file is created on first start at
`X-Plane 12/Output/preferences/SimBriefDownloader.txt`:

```
# Your SimBrief pilot ID (https://dispatch.simbrief.com/account)
simbrief_id:123456

# ICAO:path[:format]
#   path   - directory relative to X-Plane 12/Aircraft/
#   format - SimBrief download format (default: xpe = X-Plane 11/12 .fms)
B752:Boeing/757/co-routes:vm5
B753:Boeing/757/co-routes:vm5
```

- Aircraft types without an entry get an X-Plane `.fms` file in `Output/FMS plans`.
- Mapped directories must already exist. If one doesn't, the plugin reports it
  instead of creating it.
- Files are named `{ORIGIN}{DESTINATION}.{ext}`, e.g. `KJFKEGLL.fms`, and replace an
  existing file with the same name.
- If a format name is wrong, the error says so and `Log.txt` lists the formats SimBrief
  offers. `vm5` is the FlightFactor 757 V2 and `vmx` is the FlightFactor 767/777.

## Build

Requires Rust. On x86_64 Linux:

```
tools/build-dist.sh
```

This produces `dist/SimBriefDownloader/lin_x64/SimBriefDownloader.xpl`. On other hosts
the script cross-compiles with [Zig](https://ziglang.org/). Set `ZIG` if `zig` is not
on your PATH. Windows and macOS builds are not set up yet.

Run the tests with `cargo test -p simbrief-core`.

## Testing without X-Plane

`simbrief-dev` runs the plugin's download logic from the command line. It uses a fake
X-Plane directory, `target/xplane-sandbox` by default, with its config at
`Output/preferences/SimBriefDownloader.txt`. The config is created on first use.

```
cargo sb info                          # latest flight plan: aircraft, route, all formats
cargo sb fetch --type B752             # download as if a B752 were loaded
cargo sb fetch --type B752 --format xpe  # same, forcing a SimBrief format
cargo sb config                        # check the config, show where each type goes
```

- `fetch` refuses, like the plugin does, if the flight plan is for another aircraft
  type.
- The pilot ID comes from `--id`, the config's `simbrief_id`, or `SIMBRIEF_ID`.
- Use `--root <dir>` to point at a different X-Plane directory, including a real one.
- To test a mapping, create its directory first, e.g.
  `mkdir -p target/xplane-sandbox/Aircraft/Boeing/757/co-routes`.
- `info` and `fetch` make live SimBrief requests, so please don't run them in a loop.

## License

BSD 2-Clause, see [LICENSE](LICENSE).
