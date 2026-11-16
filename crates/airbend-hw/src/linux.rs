//! Linux backend over the kernel's hwmon sysfs interface (`/sys/class/hwmon`), the same files
//! lm-sensors reads. No libsensors needed.
//!
//! - `tempN_input`: millidegrees Celsius. `fanN_input`: RPM.
//! - `pwmN`: duty 0–255. `pwmN_enable`: 1 = manual; other values are driver-specific automatic
//!   modes (2 on most chips, 5 on some nct6775 modes), 0 = full speed on some drivers.
//!
//! IDs key on the hwmon's `name` and its device path, not `hwmonN`, which can renumber between
//! boots.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use airbend_core::{ControlInfo, SensorInfo, SensorKind};
use anyhow::{Context, Result};
use log::{info, warn};

use crate::{Backend, percent_to_pwm, pwm_to_percent};

const DEFAULT_ROOT: &str = "/sys/class/hwmon";

struct Sensor {
    info: SensorInfo,
    path: PathBuf,
    /// Raw value divided by this gives the unit (millidegrees → °C).
    divisor: f32,
}

struct Control {
    info: ControlInfo,
    pwm: PathBuf,
    enable: Option<PathBuf>,
    /// The `pwmN_enable` value before airbend took the output, while it holds it.
    original: Option<Original>,
}

#[derive(Clone, Copy, Debug)]
enum Original {
    Enable(u32),
    /// The driver has no `_enable` file: we can only set a duty, so release goes to full speed.
    NoEnable,
}

pub struct HwmonBackend {
    root: PathBuf,
    sensors: Vec<Sensor>,
    controls: Vec<Control>,
    restore_file: Option<PathBuf>,
}

impl HwmonBackend {
    pub fn new(restore_file: Option<PathBuf>) -> Result<HwmonBackend> {
        Self::with_root(DEFAULT_ROOT, restore_file)
    }

    pub fn with_root(root: impl Into<PathBuf>, restore_file: Option<PathBuf>) -> Result<HwmonBackend> {
        let mut b = HwmonBackend { root: root.into(), sensors: Vec::new(), controls: Vec::new(), restore_file };
        b.scan()?;
        info!(
            "hwmon: {} sensors, {} controllable outputs",
            b.sensors.len(),
            b.controls.len()
        );
        Ok(b)
    }

    fn scan(&mut self) -> Result<()> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(&self.root)
            .with_context(|| format!("reading {}", self.root.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        dirs.sort();
        let mut seen: BTreeMap<String, u32> = BTreeMap::new();
        for dir in dirs {
            let name = read_trim(&dir.join("name")).unwrap_or_else(|_| "hwmon".into());
            let dev = fs::canonicalize(dir.join("device"))
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            let mut key = match &dev {
                Some(d) => format!("{name}@{d}"),
                None => name.clone(),
            };
            // Two chips with the same name and no distinguishing device: number them.
            let n = seen.entry(key.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                key = format!("{key}#{n}");
            }
            self.scan_chip(&dir, &name, &key);
        }
        Ok(())
    }

    fn scan_chip(&mut self, dir: &Path, name: &str, key: &str) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut files: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort_by_key(|f| natural_key(f));
        for f in &files {
            if let Some(n) = strip(f, "temp", "_input") {
                let label = read_trim(&dir.join(format!("temp{n}_label"))).unwrap_or_else(|_| format!("Temp {n}"));
                self.sensors.push(Sensor {
                    info: SensorInfo {
                        id: format!("hwmon:{key}/temp{n}"),
                        label,
                        device: name.into(),
                        kind: SensorKind::Temperature,
                        value: None,
                    },
                    path: dir.join(f),
                    divisor: 1000.0,
                });
            } else if let Some(n) = strip(f, "fan", "_input") {
                let label = read_trim(&dir.join(format!("fan{n}_label"))).unwrap_or_else(|_| format!("Fan {n}"));
                self.sensors.push(Sensor {
                    info: SensorInfo {
                        id: format!("hwmon:{key}/fan{n}"),
                        label,
                        device: name.into(),
                        kind: SensorKind::Fan,
                        value: None,
                    },
                    path: dir.join(f),
                    divisor: 1.0,
                });
            } else if let Some(n) = f.strip_prefix("pwm").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                let enable = dir.join(format!("pwm{n}_enable"));
                let fan = files.contains(&format!("fan{n}_input")).then(|| format!("hwmon:{key}/fan{n}"));
                self.controls.push(Control {
                    info: ControlInfo {
                        id: format!("hwmon:{key}/pwm{n}"),
                        label: format!("PWM {n}"),
                        device: name.into(),
                        percent: None,
                        paired_fan: fan,
                        managed: false,
                        min_percent: 0.0, max_percent: 100.0, calibration_supported: true,
                    },
                    pwm: dir.join(f),
                    enable: enable.exists().then_some(enable),
                    original: None,
                });
            }
        }
    }

    fn control_mut(&mut self, id: &str) -> Result<&mut Control> {
        self.controls.iter_mut().find(|c| c.info.id == id).with_context(|| format!("no control {id}"))
    }

    /// Writes `{pwm_enable path: original value}` for every held output, so `--restore` can
    /// undo a crashed daemon's takeover.
    fn save_restore_file(&self) -> Result<()> {
        let Some(path) = &self.restore_file else { return Ok(()) };
        let map: BTreeMap<String, String> = self
            .controls
            .iter()
            .filter_map(|c| match (c.original?, &c.enable) {
                (Original::Enable(v), Some(e)) => Some((e.display().to_string(), restore_value(v).to_string())),
                (Original::NoEnable, _) => Some((c.pwm.display().to_string(), "255".into())),
                _ => None,
            })
            .collect();
        let write = || -> Result<()> {
            if map.is_empty() {
                if path.exists() {
                    fs::remove_file(path)?;
                }
                return Ok(());
            }
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&map)?)?;
            fs::rename(&tmp, path)?;
            Ok(())
        };
        write().with_context(|| format!("writing restore file {}", path.display()))
    }
}

/// Preserve firmware and full-speed modes. If an output was already manual when
/// taken (for example after a crash), full speed is the portable safe fallback;
/// automatic modes are driver-specific and may be unsupported on that header.
fn restore_value(original: u32) -> u32 {
    if original == 1 { 0 } else { original }
}

impl Backend for HwmonBackend {
    fn name(&self) -> &'static str {
        "hwmon"
    }

    fn refresh(&mut self) -> Result<()> {
        for s in &mut self.sensors {
            s.info.value = read_trim(&s.path).ok().and_then(|v| v.parse::<f32>().ok()).map(|v| v / s.divisor);
        }
        for c in &mut self.controls {
            c.info.percent = read_trim(&c.pwm).ok().and_then(|v| v.parse::<u8>().ok()).map(pwm_to_percent);
            c.info.managed = c.original.is_some();
        }
        Ok(())
    }

    fn sensors(&self) -> Vec<SensorInfo> {
        self.sensors.iter().map(|s| s.info.clone()).collect()
    }

    fn controls(&self) -> Vec<ControlInfo> {
        self.controls.iter().map(|c| c.info.clone()).collect()
    }

    fn set(&mut self, control: &str, percent: f32) -> Result<()> {
        let c = self.control_mut(control)?;
        if c.original.is_none() {
            c.original = Some(match &c.enable {
                Some(e) => {
                    let v = read_trim(e)?.parse::<u32>().with_context(|| format!("parsing {}", e.display()))?;
                    Original::Enable(v)
                }
                None => Original::NoEnable,
            });
            info!("took {control}");
        }
        // Persist the recovery record before changing the hardware, and refuse
        // takeover if it cannot be written.
        self.save_restore_file()?;
        let c = self.control_mut(control)?;
        if let Some(e) = &c.enable
            && read_trim(e)? != "1"
        {
            fs::write(e, "1").with_context(|| format!("taking manual control via {}", e.display()))?;
        }
        c.info.managed = true;
        fs::write(&c.pwm, percent_to_pwm(percent).to_string()).with_context(|| format!("writing {}", c.pwm.display()))
    }

    fn release(&mut self, control: &str) -> Result<()> {
        let c = self.control_mut(control)?;
        let Some(orig) = c.original else { return Ok(()) };
        match (orig, &c.enable) {
            (Original::Enable(v), Some(e)) => fs::write(e, restore_value(v).to_string()).with_context(|| format!("restoring {}", e.display())),
            _ => fs::write(&c.pwm, "255").with_context(|| format!("writing {}", c.pwm.display())),
        }?;
        c.original = None;
        c.info.managed = false;
        info!("released {control}");
        self.save_restore_file()
    }

    fn release_all(&mut self) {
        let held: Vec<String> = self.controls.iter().filter(|c| c.original.is_some()).map(|c| c.info.id.clone()).collect();
        for id in held {
            if let Err(e) = self.release(&id) {
                warn!("releasing {id}: {e:#}");
            }
        }
    }
}

impl Drop for HwmonBackend {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Undoes a previous run's takeover from its restore file. Used by `airbendd --restore`
/// (systemd's `ExecStopPost`), which runs even when the daemon was killed.
pub fn restore_from_file(path: &Path) -> Result<usize> {
    let Ok(bytes) = fs::read(path) else { return Ok(0) };
    let map: BTreeMap<String, String> = serde_json::from_slice(&bytes).context("parsing restore file")?;
    let mut n = 0;
    let mut failed = BTreeMap::new();
    for (file, value) in &map {
        // Only ever write into hwmon's sysfs, whatever the file says.
        if !(file.starts_with("/sys/class/hwmon/") || file.starts_with("/sys/devices/")) {
            warn!("restore: skipping {file}: not a sysfs path");
            continue;
        }
        match fs::write(file, value) {
            Ok(()) => n += 1,
            Err(e) => {
                warn!("restore: {file}: {e}");
                failed.insert(file.clone(), value.clone());
            }
        }
    }
    if !failed.is_empty() {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&failed)?)?;
        fs::rename(tmp, path)?;
        anyhow::bail!("could not restore {} outputs; recovery record retained", failed.len());
    }
    fs::remove_file(path)?;
    Ok(n)
}

fn read_trim(p: &Path) -> Result<String> {
    Ok(fs::read_to_string(p)?.trim().to_string())
}

/// `temp12_input` with ("temp", "_input") → Some("12").
fn strip<'a>(f: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let n = f.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(n)
}

/// Sorts `temp2` before `temp10`.
fn natural_key(f: &str) -> (String, u32, String) {
    let start = f.find(|c: char| c.is_ascii_digit()).unwrap_or(f.len());
    let end = f[start..].find(|c: char| !c.is_ascii_digit()).map_or(f.len(), |e| start + e);
    (f[..start].to_string(), f[start..end].parse().unwrap_or(0), f[end..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake hwmon tree: one chip with two temps, a fan and a pwm with an enable file.
    fn fake_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("airbend-hwmon-{}-{}", std::process::id(), rand_suffix()));
        let chip = root.join("hwmon3");
        fs::create_dir_all(&chip).unwrap();
        for (f, v) in [
            ("name", "it8689\n"),
            ("temp1_input", "45000\n"),
            ("temp1_label", "CPU\n"),
            ("temp10_input", "38500\n"),
            ("fan1_input", "1200\n"),
            ("pwm1", "128\n"),
            ("pwm1_enable", "2\n"),
        ] {
            fs::write(chip.join(f), v).unwrap();
        }
        root
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn scans_reads_takes_and_restores() {
        let root = fake_tree();
        let restore = root.join("restore.json");
        let mut b = HwmonBackend::with_root(&root, Some(restore.clone())).unwrap();
        b.refresh().unwrap();

        let s = b.sensors();
        let ids: Vec<_> = s.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["hwmon:it8689/fan1", "hwmon:it8689/temp1", "hwmon:it8689/temp10"]);
        assert_eq!(s[1].label, "CPU");
        assert_eq!(s[1].value, Some(45.0));
        assert_eq!(s[0].value, Some(1200.0));

        let c = &b.controls()[0];
        assert_eq!(c.id, "hwmon:it8689/pwm1");
        assert_eq!(c.paired_fan.as_deref(), Some("hwmon:it8689/fan1"));
        assert!((c.percent.unwrap() - 50.2).abs() < 0.1);

        let chip = root.join("hwmon3");
        b.set("hwmon:it8689/pwm1", 40.0).unwrap();
        assert_eq!(read_trim(&chip.join("pwm1_enable")).unwrap(), "1");
        assert_eq!(read_trim(&chip.join("pwm1")).unwrap(), "102");
        assert!(restore.exists(), "takeover is recorded before the first write");

        b.release_all();
        assert_eq!(read_trim(&chip.join("pwm1_enable")).unwrap(), "2");
        assert!(!restore.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["temp10_input", "temp2_input", "temp1_input"];
        v.sort_by_key(|f| natural_key(f));
        assert_eq!(v, ["temp1_input", "temp2_input", "temp10_input"]);
    }

    #[test]
    fn refuses_takeover_without_a_recovery_record() {
        let root = fake_tree();
        fs::write(root.join("blocked"), "file, not a directory").unwrap();
        let mut b = HwmonBackend::with_root(&root, Some(root.join("blocked/restore.json"))).unwrap();
        assert!(b.set("hwmon:it8689/pwm1", 20.0).is_err());
        assert_eq!(read_trim(&root.join("hwmon3/pwm1_enable")).unwrap(), "2");
        assert_eq!(read_trim(&root.join("hwmon3/pwm1")).unwrap(), "128");
        drop(b);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_release_retains_the_record_and_can_be_retried() {
        let root = fake_tree();
        let restore = root.join("restore.json");
        let mut b = HwmonBackend::with_root(&root, Some(restore.clone())).unwrap();
        b.set("hwmon:it8689/pwm1", 40.0).unwrap();
        let enable = root.join("hwmon3/pwm1_enable");
        fs::remove_file(&enable).unwrap();
        fs::create_dir(&enable).unwrap();
        assert!(b.release("hwmon:it8689/pwm1").is_err());
        assert!(restore.exists());
        assert!(b.controls()[0].managed);
        fs::remove_dir(&enable).unwrap();
        fs::write(&enable, "1").unwrap();
        b.release("hwmon:it8689/pwm1").unwrap();
        assert_eq!(read_trim(&enable).unwrap(), "2");
        assert!(!restore.exists());
        drop(b);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_full_speed_mode_and_recovers_manual_mode_at_full_speed() {
        for original in ["0", "1"] {
            let root = fake_tree();
            let enable = root.join("hwmon3/pwm1_enable");
            fs::write(&enable, original).unwrap();
            let mut b = HwmonBackend::with_root(&root, Some(root.join("restore.json"))).unwrap();
            b.set("hwmon:it8689/pwm1", 20.0).unwrap();
            b.release("hwmon:it8689/pwm1").unwrap();
            assert_eq!(read_trim(&enable).unwrap(), "0");
            drop(b);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
