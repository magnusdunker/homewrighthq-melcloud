# MELCloud for HomewrightHQ

An external integration for [HomewrightHQ](https://github.com/magnusdunker/homewrighthq) for Mitsubishi Electric air conditioners (air-to-air) and Ecodan heat pumps (air-to-water) through MELCloud.

## Installation

In HomewrightHQ: **Settings → Integrations → From GitHub**. Paste

```
https://github.com/magnusdunker/homewrighthq-melcloud
```

and press **Install**. Then add **MELCloud** under **Add integration**, and log in with the email and password you use in the MELCloud app.

## What you get

An air-to-air unit becomes a device with:

- a climate entity with modes (heat, cool, dry, fan, auto), temperature and fan speed (auto and 1 up to the number of steps the unit has)
- vertical vane (auto, 1-5, swing) and horizontal vane (auto, 1-5, split, swing), where the unit has one
- room temperature, and outdoor temperature where MELCloud reports it
- total energy use in kWh on units with an energy meter

An air-to-water unit (Ecodan) becomes a device with:

- a hot water tank with target temperature and the modes `auto` and `force_hot_water`
- a climate entity per zone (zone 2, where the unit has it) with heat, cool where the unit can cool, and the presets `room` (room temperature), `flow` (flow temperature) and `curve` (heating curve)
- a setting for the heating flow temperature per zone
- room temperature per zone, outdoor temperature, flow, return and hot water temperature
- status: `idle`, `hot_water`, `heating`, `cooling`, `defrost`, `standby` or `legionella`

Turning off an air-to-water unit turns off the whole unit, hot water included, because MELCloud has only one on/off switch for the unit.

## Updates and MELCloud's limits

MELCloud blocks accounts that ask too often. The integration reads each unit every five minutes and the list of units (energy, flow and return) once an hour. Under **Options** on the integration, the time between updates can be set from 3 to 60 minutes. If MELCloud answers 429 (too many requests), the integration pauses for at least 30 minutes, or as long as MELCloud asks.

After a command the integration waits 90 seconds before it reads the unit again, because MELCloud can answer with the old values for a while. The unit itself picks up the command at its next connection to MELCloud, usually within a minute.

## Not tested against the real MELCloud

The integration is written from how MELCloud's web API is described in pymelcloud and Home Assistant's MELCloud integration, and tested against a fake MELCloud with responses shaped like the real ones. These things are not checked against real units:

- The `EffectiveFlags` values for air-to-water (tank, zone temperature, zone mode, flow). The values for air-to-air are well known.
- The codes for status (`OperationMode` 0-6) and zone modes (`OperationModeZone1/2` 0-4) on air-to-water.
- Whether the flow and return temperatures are in `Device/Get`; if not, the values from the list of units are used, which is fetched only once an hour.
- Which fields say that an air-to-air unit can dry or run auto (`ModelSupportsDryMode`, `ModelSupportsAuto`). If they are missing, the mode is shown.
- How often MELCloud allows requests. Five minutes is a cautious guess.
- The `AppVersion` sent at login.

Ventilation units (Lossnay, `DeviceType` 3) and MELCloud Home (the new app) are not included.

## Development

Built in Rust on the `thehome-plugin` SDK from [homewright-example-integration-rust](https://github.com/magnusdunker/homewrighthq-example-integration-rust).

- `src/api.rs`: login and MELCloud's web API
- `src/model.rs`: MELCloud's data and how it becomes states and commands
- `src/lib.rs`: the connection to HomewrightHQ
- `tests/melcloud.rs`: tests against a fake MELCloud with the responses in `tests/fixtures`, and a fake HomewrightHQ in `tests/host`

```
cargo test
cargo build --release
```

A new version is released with a tag, e.g. `git tag v0.1.0 && git push origin v0.1.0`. The workflow builds for Windows, Linux (x86_64 and arm64, static musl) and macOS and puts the binaries and `homewright.json` on the release.

## License

MIT
