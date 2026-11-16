# airbend

Fan control for Linux: temperature curves for motherboard fans and NVIDIA GPU fans, with a
native GPU-rendered UI built on [Rune](https://github.com/anomalyco/rune). MIT licensed, written
from scratch in Rust.

## Layout

| Crate | What it is |
|---|---|
| `airbend-core` | Curves (graph, linear, flat, trigger, mix), tuning (hysteresis, response time, steps, minimum, start kick, zero-RPM), config validation and the control engine. Pure, no I/O. |
| `airbend-hw` | Hardware backends: Linux hwmon (`/sys/class/hwmon`), NVIDIA NVML, and a mock for development. |
| `airbend-ipc` | The daemon ↔ UI protocol: JSON lines over a Unix socket. |
| `airbend-daemon` | `airbendd`: runs as root, owns the hardware, runs the loop, serves the socket. |
| `airbend-ui` | `airbend`: the Rune desktop app. Never touches hardware. |

## Features

See [the complete feature outline](FEATURES.md) for controls, editing, hardware support,
safety behavior, installation, development tools, and current limitations.

- Curves: graph (drag points), linear, flat, trigger, mix; hysteresis, response time.
- Per-fan tuning: minimum, stop below minimum, start kick, step up/down, offset, manual override.
- **Calibration**: per output, finds which tachometer it drives (headers and tach inputs don't
  always share a number), top RPM, the lowest duty that keeps the fan spinning and the duty it
  needs to start; one click turns that into the fan's minimum and start kick. About two minutes,
  aborted if anything reaches 85 °C, a temperature or the paired tachometer becomes
  unreadable, or an output write fails. Only one output can be calibrated at a time.
- **Renaming** fans, sensors and curves.
- **Hide unused outputs** on Controls hides inactive headers with no detected fan. Managed,
  enabled, calibrated and NVIDIA fans stay visible even at zero RPM. Saved with UI preferences.
- **NVIDIA GPUs**: reads GPU temperature and each fan's RPM, and controls each fan through
  NVML on Wayland or X11. Requires the NVIDIA driver and `nvidia-utils`; the library is loaded
  only when available. Driver limits are enforced (30–100% on the tested RTX 4070 SUPER).
  Stop/start calibration is unavailable for these fans. At 85 °C or if GPU temperature
  cannot be read, manual fan requests are overridden to 100%.
- **Themes**: Follow Omarchy (reads `~/.local/state/omarchy/current/theme/colors.toml` and follows
  `omarchy theme set` live) or Dark. Saved in `~/.config/airbend/ui.toml`.

## Safety model

- The UI edits a draft; **Apply** sends it to the daemon, which validates before saving or using it.
- A curve whose sensor can't be read drives its fans to **100 %**.
- Calibration results belong to the daemon; applying an older UI draft preserves them.
- On SIGINT/SIGTERM, or a panic, the daemon hands every fan back to the firmware.
  Outputs previously in manual mode fall back to full speed.
- Before taking a fan it records the fan's original mode in `/run/airbend/restore.json`; the
  systemd unit's `ExecStopPost=airbendd --restore` replays it even after SIGKILL or OOM.
  Takeover is refused if the record cannot be saved; failed restores retain it for retry.
- NVIDIA takeovers are recorded separately in `/run/airbend/restore.nvidia.json`; shutdown
  and `ExecStopPost` return those fans to NVIDIA's automatic control, including after SIGKILL.
- Socket access is limited to root and one group (`wheel` by default), mode 0660.

## Try it without touching fans

```sh
cargo run -p airbend-daemon -- --mock --socket /tmp/airbend.sock --config /tmp/airbend.toml
AIRBEND_SOCKET=/tmp/airbend.sock cargo run -p airbend-ui
```

## Install on Arch / Omarchy

```sh
sudo pacman -S --needed lm_sensors libxkbcommon wayland
sudo sensors-detect            # finds your Super I/O chip and the driver it needs
cd dist/arch && makepkg -si    # builds this checkout into an `airbend` package
sudo systemctl enable --now airbendd
```

Then open airbend from the app launcher. `sudo pacman -R airbend` removes it again.

### No fans listed?

The motherboard's fan headers are driven by a Super I/O chip, and its kernel driver has to be
loaded before any tool can see `pwmN` files. `sensors-detect` names it. Common cases:

- **Gigabyte** (ITE IT86xx/IT87xx): on the tested B650I AORUS ULTRA, the in-tree `it87`
  driver detected the IT8689E but writes to `pwmN` did not change fan speeds. Installing
  `it87-dkms-git` from the AUR (Frank Crawford's driver) enabled control through MMIO. Then
  `echo it87 | sudo tee /etc/modules-load.d/it87.conf` and
  `echo "options it87 ignore_resource_conflict=1" | sudo tee /etc/modprobe.d/it87.conf`
  (Gigabyte's ACPI claims the chip's ports, so the driver must be told to share them).
- **ASUS / MSI / ASRock** (Nuvoton NCT67xx): `sudo modprobe nct6775` (in-tree), then the same
  `modules-load.d` trick to keep it.
- `gigabyte_wmi` and `asus_wmi_sensors` give temperatures only; they can't drive fans.
- AMD GPUs expose `pwm1` under `amdgpu` (discrete cards; integrated graphics have no fan).

Check with `ls /sys/class/hwmon/*/pwm[0-9]`.

## Development

```sh
cargo test --workspace
# Headless UI renders with demo data (no daemon needed):
cargo run -p airbend-ui -- --snap curves.png --scene curves   # scenes: controls curves mix sensors dirty disconnected
cargo run -p airbend-ui -- --tree tree.json --scene controls
```

Rune is pre-release and pinned to one `dev` rev in the root `Cargo.toml`; when bumping it, update
the `[patch.crates-io]` entries to the same rev (see Rune's root `Cargo.toml`).

## Roadmap

1. ~~Linux MVP: hwmon, daemon, curve editor~~
2. ~~Fan calibration, renaming, Omarchy theme following~~
3. Profiles (quiet / performance), a tray item
4. Windows: service + named pipe, LibreHardwareMonitor sidecar over PawnIO, NVAPI/ADLX GPUs
5. Plugin API for extra sensors and controllers (liquidctl AIOs, etc.)
