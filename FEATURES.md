# Airbend features

Current Linux MVP: application version 0.1.0, Arch package release 0.1.0-5.

## Hardware and monitoring

- Discover temperature sensors, fan tachometers, and PWM outputs through Linux hwmon.
- Control motherboard CPU, case, exhaust, and accessory fans when their kernel driver exposes
  writable PWM controls. A splitter's fans share one output and may expose only one tachometer.
- Read temperatures from CPU, SSD, motherboard, and GPU devices exposed by the supported backends.
- Support AMD GPU fans through hwmon when the driver exposes a PWM output.
- Support NVIDIA GPU temperature, individual fan RPM readings, and independent fan controls
  through NVML. Works without an X11 fan-control session, including on Wayland.
- Load NVML dynamically, so NVIDIA support adds no mandatory dependency on other machines.
- Enforce NVIDIA's reported fan-speed limits. The tested RTX 4070 SUPER reports 30–100%.
- Use stable device IDs rather than changing `hwmonN` numbers for saved assignments and names.
- Show live duty percentages, paired RPM, firmware/Airbend ownership, sensor readings,
  connection status, and daemon warnings. The UI polls the daemon about once a second.

## Fan curves

| Curve | Behavior |
|---|---|
| Graph | Piecewise linear temperature-to-duty curve with draggable control points. |
| Linear | Interpolate between chosen temperatures and fan speeds. |
| Flat | Hold a fixed percentage. |
| Trigger | Switch between idle and load speeds with separate temperature thresholds. |
| Mix | Combine other curves using maximum, minimum, average, or sum. |

- Assign a curve to each output; several fans can share a curve.
- Combine CPU, SSD, and GPU cooling demands into an exhaust curve.
- Add, rename, and delete curves. Deleting a curve removes its references from fans and mixes.
- Drag graph points to change temperature and duty; click empty graph space to add a point;
  double-click a point to remove it. Points remain ordered, and at least one is retained.
- Show live temperature/demand markers on the curve graph.
- Set hysteresis and a response-time averaging window to reduce fan-speed fluctuations.
- Choose whether hysteresis applies only while cooling or in both temperature directions.

## Per-fan controls

- Enable Airbend control or return an output to firmware/driver automatic control.
- Select a curve or use a manual percentage override.
- Set minimum duty, a start kick, and optional stop-below-minimum behavior for supported outputs.
- Limit how quickly duty rises and falls, and apply an offset to curve demand.
- Configure a maximum-duty cap in the TOML configuration.
- Respect NVIDIA's driver limits; GPU controls omit unsupported stop/start calibration settings.

## Motherboard fan calibration

- Run calibration on one output at a time, with progress reporting and cancellation.
- Detect which tachometer responds to the output, including mismatched PWM/tachometer numbering.
- Restrict pairing to the output's controller so a changing GPU fan cannot steal the pairing.
- Measure full-speed RPM, the lowest measured duty that keeps the fan spinning, restart duty,
  and a duty/RPM sample curve. Typical duration is roughly one to two minutes.
- Offer a button to copy suggested minimum and start-kick values into the editing draft,
  with a margin above the measured thresholds.
- Save results through the daemon, including their measurement time; an older UI draft cannot
  overwrite a calibration that completed while the user was editing.
- Abort on any monitored temperature reaching 85 °C, unavailable temperature monitoring,
  a missing identified tachometer, an output-write failure, or a timeout.
- Report when no tachometer responds; do not offer unverified limits in that case.
- NVIDIA fans use driver-provided limits instead of a stop/start calibration sweep.

## Desktop interface and preferences

- Native GPU-rendered Rune interface with Controls, Curves, Sensors, and Settings pages.
- Rename fan outputs, temperature sensors, tachometers, and curves. Clear an output/sensor
  rename to restore its hardware label.
- Hide unused outputs with a switch on Controls. Inactive headers with no detected fan are
  hidden; enabled, managed, calibrating, calibrated, and NVIDIA outputs remain visible even
  at zero RPM. Switching it off reveals the headers again.
- Choose Dark or Follow Omarchy. Follow Omarchy reads the active palette and checks for
  changes about every two seconds; it falls back to Dark when no Omarchy theme is available.
- Persist theme and output-visibility preferences per user in
  `$XDG_CONFIG_HOME/airbend/ui.toml`, normally `~/.config/airbend/ui.toml`.
- Edit a draft with Apply and Revert. Fan/curve/sensor renames go through Apply; display
  preferences are saved immediately.
- Preserve local draft edits when the daemon publishes calibration updates, and keep edits
  made during a save separate from the configuration acknowledged by the daemon.
- Reconnect to the daemon automatically and show connection/save errors in the interface.

## Safety and persistence

- Run hardware control in the privileged `airbendd` daemon; the desktop UI never writes hardware.
- Validate configuration before accepting it, including curve references, mix cycles, graph
  ordering, duty bounds, and the control-loop interval.
- Save the shared configuration to `/etc/airbend/config.toml` through a temporary-file rename.
  The TOML configuration includes curves, assignments, tuning, labels, and calibration results.
- Configure the daemon's polling interval between 100 ms and 10 seconds in TOML.
- Drive affected fans at 100% immediately when a curve's temperature sensor cannot be read.
- Force managed NVIDIA fans to 100% at 85 °C or when the GPU temperature cannot be read,
  even when using a flat curve or manual override.
- Restore original motherboard modes on shutdown, with full speed as the safe fallback for
  outputs previously in manual mode or without an automatic-mode interface.
- Restore NVIDIA fans to the driver's default automatic policy on shutdown.
- Record takeovers before changing hardware. Refuse takeover if a recovery record cannot
  be saved, and retain failed recovery records for retry.
- Run systemd recovery after a crash, SIGKILL, or OOM using separate hwmon and NVIDIA journals.
- Restrict the Unix control socket to root and a selected group, `wheel` by default, mode 0660.
- Run as a hardened systemd service, with automatic restart after failures and boot enablement.

## Installation and development

- Arch PKGBUILD builds and tests this checkout, then installs the app, daemon, launcher entry,
  icon, systemd service, license, example configuration, and README.
- Supports configurable daemon paths/group and the `AIRBEND_SOCKET` override for local sessions.
- Provides a simulated backend with drifting temperatures, fan inertia, stall/restart behavior,
  and deliberately mismatched PWM/tachometer numbering.
- Provides deterministic UI scenes for themes, renaming, calibration, NVIDIA controls,
  unused outputs, curve editing, sensors, and disconnection.
- Supports headless screenshots and UI-tree inspection in debug builds. Rune's optional
  `tooling` feature is required for these commands in release builds.
- Includes 50 automated tests covering the engine, configuration, calibration, recovery,
  NVIDIA safety/limits, IPC, and interactions with the actual UI components.
- Uses a Rust workspace with separate core, hardware, IPC, daemon, and UI crates; the Rune
  revision and Cargo dependencies are pinned for reproducible dependency selection.
- MIT license.

## Current scope

Linux is implemented. Windows support, profiles, a tray item, and a general sensor/controller
plugin API remain on the roadmap. Hardware discovery happens when the daemon starts; loading
a new driver or changing the available devices can require restarting the daemon. Fan control
depends on the hardware and driver exposing a supported interface, and RPM availability can
vary by device. A zero or missing tachometer reading alone does not prove a header is empty.
