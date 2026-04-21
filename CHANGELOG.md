# Changelog

## v0.1.13

### BREAKING

- **security(upgrade)**: `fr upgrade` now aborts if the release has no `checksums.txt`. Previously it warned and proceeded. Pass `--allow-unsigned` to opt in to the old behavior (for private/offline releases). Same policy applies to `install.sh`; honor env var `FORGE_ALLOW_UNSIGNED=1` or pass `--allow-unsigned`.
- **security(install.sh)**: rustup bootstrap no longer uses `curl | sh`; the installer is downloaded to a temp file first and then executed, protecting against truncated-response execution.

### Added

- **feat(cli)**: `fr cache clear [--service NAME]` wipes the on-disk command cache. Use `--service gateway/api` to scope to one service (or a target pattern); without the flag, clears the whole workspace cache root.
- **feat(cli)**: `-v` / `-vv` now raises the default log level (`forge_cli=info` / `debug`) for every subcommand, not only `fr run`. Explicit `RUST_LOG` still wins.
- **docs(cli)**: `--attach` help now spells out the `--attach=VALUE` single-value trap; use the space-separated form for multiple targets.

### Fixed

- **fix(up)**: `fr up` now bails immediately when any service in a topological level fails its health check, instead of continuing to start dependents. Error message lists the failed services.
- **fix(run)**: parallel `fr run` fail-fast no longer waits behind slow sibling tasks — switched to `JoinSet` so `cancel.cancel()` fires the moment any task reports failure. Results are re-sorted by stable topo order for deterministic JSON output.
- **fix(health)**: HTTP health check now re-confirms port ownership after a healthy response (re-queries `detect_listening_ports` and checks the PID) to eliminate a TOCTOU where the detected port could be rebound by another process before the probe returned.
- **fix(supervisor)**: concurrency & lifecycle hardening — `kill_existing_supervisor` no longer blocks the tokio worker with `std::thread::sleep`; `ProjectConfig` is shared via `Arc` instead of deep-cloned per connection; supervisor and service PID/port files are written atomically (temp + rename) to avoid zero-length files after a crash; `ctrlc::set_handler` is dispatched through a global `OnceLock` so repeat registrations work.
- **fix(supervisor)**: `fr down` now captures each `down_cmd` exit status, aggregates per-service teardown failures, and returns non-zero on failure instead of masking them.
- **fix(process/log)**: `is_process_alive` and the stale-service-kill path now log a warning when a pid doesn't fit in `i32` instead of silently returning false. Log broadcast channel capacity (default 10000) is tunable via `FORGE_LOG_CAP`. HTTP-vs-cmd probe dispatch was deduplicated into a shared `probe_once`.

## v0.1.12

- fix(platform): fix lsof port parsing on macOS 26.4 beta where `-sTCP:LISTEN` still appends `(LISTEN)` token; now scans fields from end for first `addr:port` pattern
- fix(health): HTTP health checks no longer fall back to configured port_hint; prevents false-positive when another service on the configured port responds with HTTP 200
- fix(server): `fr ps` dynamic port detection uses live `detect_listening_ports` instead of cached `detected_port.or(config.port)`; no config-port fallback
- fix(upgrade): use semver comparison to prevent downgrade when local version is ahead of latest release

## v0.1.11

- fix(port): remove kill_port_listeners — port config is a hint, not an exclusive claim; services handle port conflicts themselves (906d6a2)
- fix(port): propagate health-check confirmed port to avoid re-running lsof detection that could fall back to config port (dee2b1b)

## v0.1.10

- fix(server): retry port detection after health check with 5s backoff (b1ea150)

## v0.1.9

- fix(platform): detect listening ports across full process tree for shell-wrapped services (sh → yarn → node) (1eb9dac)
- test: comprehensive unit test coverage across 6 modules (e777bd7)

## v0.1.8

- fix: use actual listening port for health check instead of configured port (4e6528e)
- feat: docker compose port detection and live health checks on status (d7dc3bf)

## v0.1.6

- feat: docker compose port detection and live health checks on status (d7dc3bf)
- style: unify LiveList table borders to match UTF8_BORDERS_ONLY preset (f4f7c8d)

## v0.1.5

- feat: adaptive table width and live startup progress display (380f8ff)

## v0.1.4

- fix: disable table content wrapping to prevent line breaks on narrow terminals (f312f52)
- style: switch table preset to UTF8_BORDERS_ONLY for cleaner output (e0e9a1a)
- fix: set current_dir for health check cmd commands (60d2dd5)
- feat: add Windows support (x86_64-pc-windows-msvc) (d444cb8)
