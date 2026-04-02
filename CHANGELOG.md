# Changelog

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
