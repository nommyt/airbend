//! NVIDIA fan control through NVML, including Wayland sessions. Load the library at
//! runtime so machines without NVIDIA need no additional dependency or driver.
//! ABI declarations follow NVIDIA's nvml.h (NVIDIA/go-nvml/pkg/nvml/nvml.h).

use std::collections::BTreeSet;
use std::ffi::{CStr, c_char, c_void};
use std::fs;
use std::path::{Path, PathBuf};

use airbend_core::{ControlInfo, SensorInfo, SensorKind};
use anyhow::{Context, Result, bail};
use libloading::Library;
use log::warn;

use crate::Backend;

type Device = *mut c_void;
type Status = u32;
type Init = unsafe extern "C" fn() -> Status;
type GetCount = unsafe extern "C" fn(*mut u32) -> Status;
type GetHandle = unsafe extern "C" fn(u32, *mut Device) -> Status;
type GetString = unsafe extern "C" fn(Device, *mut c_char, u32) -> Status;
type GetUint = unsafe extern "C" fn(Device, *mut u32) -> Status;
type GetIndexed = unsafe extern "C" fn(Device, u32, *mut u32) -> Status;
type GetLimits = unsafe extern "C" fn(Device, *mut u32, *mut u32) -> Status;
type SetSpeed = unsafe extern "C" fn(Device, u32, u32) -> Status;
type Restore = unsafe extern "C" fn(Device, u32) -> Status;
type ErrorString = unsafe extern "C" fn(Status) -> *const c_char;
type GetRpm = unsafe extern "C" fn(Device, *mut FanSpeed) -> Status;

#[repr(C)]
struct FanSpeed {
    version: u32,
    fan: u32,
    speed: u32,
}

#[derive(Clone)]
struct Gpu {
    handle: usize,
    uuid: String,
    name: String,
    fans: u32,
    min: u32,
    max: u32,
}

trait NvmlApi: Send {
    fn devices(&self) -> Result<Vec<Gpu>>;
    fn temperature(&self, gpu: usize) -> Result<u32>;
    fn percent(&self, gpu: usize, fan: u32) -> Result<u32>;
    fn rpm(&self, gpu: usize, fan: u32) -> Result<u32>;
    fn set(&self, gpu: usize, fan: u32, percent: u32) -> Result<()>;
    fn restore(&self, gpu: usize, fan: u32) -> Result<()>;
}

struct Nvml {
    // Function pointers must never outlive this library.
    _library: Library,
    shutdown: Init,
    count: GetCount,
    handle: GetHandle,
    uuid: GetString,
    name: GetString,
    fans: GetUint,
    limits: GetLimits,
    temperature: GetIndexed,
    percent: GetIndexed,
    rpm: Option<GetRpm>,
    set: SetSpeed,
    restore: Restore,
    error: ErrorString,
}

impl Nvml {
    fn load() -> Result<Self> {
        // NVML is thread-safe; handles are stored as addresses and used only by
        // the owning backend. All signatures match the C ABI in nvml.h.
        unsafe {
            let library = Library::new("libnvidia-ml.so.1")?;
            let init: Init = *library.get(b"nvmlInit_v2\0")?;
            let api = Self {
                shutdown: *library.get(b"nvmlShutdown\0")?,
                count: *library.get(b"nvmlDeviceGetCount_v2\0")?,
                handle: *library.get(b"nvmlDeviceGetHandleByIndex_v2\0")?,
                uuid: *library.get(b"nvmlDeviceGetUUID\0")?,
                name: *library.get(b"nvmlDeviceGetName\0")?,
                fans: *library.get(b"nvmlDeviceGetNumFans\0")?,
                limits: *library.get(b"nvmlDeviceGetMinMaxFanSpeed\0")?,
                temperature: *library.get(b"nvmlDeviceGetTemperature\0")?,
                percent: *library.get(b"nvmlDeviceGetFanSpeed_v2\0")?,
                rpm: library.get(b"nvmlDeviceGetFanSpeedRPM\0").ok().map(|s| *s),
                set: *library.get(b"nvmlDeviceSetFanSpeed_v2\0")?,
                restore: *library.get(b"nvmlDeviceSetDefaultFanSpeed_v2\0")?,
                error: *library.get(b"nvmlErrorString\0")?,
                _library: library,
            };
            api.check(init())?;
            Ok(api)
        }
    }

    fn check(&self, status: Status) -> Result<()> {
        if status != 0 {
            let msg = unsafe { CStr::from_ptr((self.error)(status)) }.to_string_lossy();
            bail!("NVML: {msg} ({status})");
        }
        Ok(())
    }

    fn string(&self, device: Device, get: GetString) -> Result<String> {
        let mut bytes = [0u8; 128];
        self.check(unsafe { get(device, bytes.as_mut_ptr().cast(), bytes.len() as u32) })?;
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

impl NvmlApi for Nvml {
    fn devices(&self) -> Result<Vec<Gpu>> {
        let mut count = 0;
        self.check(unsafe { (self.count)(&mut count) })?;
        let mut gpus = Vec::new();
        for i in 0..count {
            let mut handle = std::ptr::null_mut();
            self.check(unsafe { (self.handle)(i, &mut handle) })?;
            let (mut fans, mut min, mut max) = (0, 0, 100);
            if unsafe { (self.fans)(handle, &mut fans) } != 0 {
                continue;
            }
            // If the driver cannot provide valid bounds, expose temperature only.
            if fans > 0
                && (unsafe { (self.limits)(handle, &mut min, &mut max) } != 0
                    || min > max
                    || max > 100)
            {
                fans = 0;
            }
            gpus.push(Gpu {
                handle: handle as usize,
                uuid: self.string(handle, self.uuid)?,
                name: self.string(handle, self.name)?,
                fans,
                min,
                max,
            });
        }
        Ok(gpus)
    }
    fn temperature(&self, gpu: usize) -> Result<u32> {
        let mut value = 0;
        self.check(unsafe { (self.temperature)(gpu as Device, 0, &mut value) })?;
        Ok(value)
    }
    fn percent(&self, gpu: usize, fan: u32) -> Result<u32> {
        let mut value = 0;
        self.check(unsafe { (self.percent)(gpu as Device, fan, &mut value) })?;
        Ok(value)
    }
    fn rpm(&self, gpu: usize, fan: u32) -> Result<u32> {
        let get = self.rpm.context("NVML RPM reporting unavailable")?;
        let mut value = FanSpeed {
            version: (std::mem::size_of::<FanSpeed>() as u32) | (1 << 24),
            fan,
            speed: 0,
        };
        self.check(unsafe { get(gpu as Device, &mut value) })?;
        Ok(value.speed)
    }
    fn set(&self, gpu: usize, fan: u32, percent: u32) -> Result<()> {
        self.check(unsafe { (self.set)(gpu as Device, fan, percent) })
    }
    fn restore(&self, gpu: usize, fan: u32) -> Result<()> {
        self.check(unsafe { (self.restore)(gpu as Device, fan) })
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
        }
    }
}

struct Fan {
    gpu: usize,
    index: u32,
    info: ControlInfo,
}

pub struct NvidiaBackend {
    api: Box<dyn NvmlApi>,
    gpus: Vec<Gpu>,
    fans: Vec<Fan>,
    sensors: Vec<SensorInfo>,
    restore_file: Option<PathBuf>,
}

impl NvidiaBackend {
    pub fn new(restore_file: Option<PathBuf>) -> Result<Self> {
        Self::with_api(Box::new(Nvml::load()?), restore_file)
    }
    fn with_api(api: Box<dyn NvmlApi>, restore_file: Option<PathBuf>) -> Result<Self> {
        let gpus = api.devices()?;
        let fans = gpus
            .iter()
            .flat_map(|g| {
                (0..g.fans).map(move |i| Fan {
                    gpu: g.handle,
                    index: i,
                    info: ControlInfo {
                        id: format!("nvidia:{}/fan{}", g.uuid, i + 1),
                        label: format!("GPU fan {}", i + 1),
                        device: g.name.clone(),
                        percent: None,
                        paired_fan: Some(format!("nvidia:{}/rpm{}", g.uuid, i + 1)),
                        managed: false,
                        min_percent: g.min as f32,
                        max_percent: g.max as f32,
                        calibration_supported: false,
                    },
                })
            })
            .collect();
        Ok(Self {
            api,
            gpus,
            fans,
            sensors: Vec::new(),
            restore_file,
        })
    }
    fn record(&self) -> Result<()> {
        let Some(path) = &self.restore_file else {
            return Ok(());
        };
        write_record(
            path,
            &self
                .fans
                .iter()
                .filter(|f| f.info.managed)
                .map(|f| f.info.id.clone())
                .collect(),
        )
    }
}

fn write_record(path: &Path, ids: &BTreeSet<String>) -> Result<()> {
    if ids.is_empty() {
        if path.exists() {
            fs::remove_file(path)?;
        }
    } else {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(ids)?)?;
        fs::rename(tmp, path)?;
    }
    Ok(())
}

fn bounded_duty(percent: f32, min: f32, max: f32, temp: Option<f32>) -> u32 {
    if !percent.is_finite() || temp.is_none_or(|t| !t.is_finite() || t >= 85.0) {
        return 100;
    }
    percent.clamp(min, max).round() as u32
}

impl Backend for NvidiaBackend {
    fn name(&self) -> &'static str {
        "nvidia"
    }
    fn refresh(&mut self) -> Result<()> {
        self.sensors.clear();
        for g in &self.gpus {
            self.sensors.push(SensorInfo {
                id: format!("nvidia:{}/temp1", g.uuid),
                label: "GPU temperature".into(),
                device: g.name.clone(),
                kind: SensorKind::Temperature,
                value: self.api.temperature(g.handle).ok().map(|v| v as f32),
            });
        }
        for f in &mut self.fans {
            f.info.percent = self.api.percent(f.gpu, f.index).ok().map(|v| v as f32);
            self.sensors.push(SensorInfo {
                id: f.info.paired_fan.clone().unwrap(),
                label: f.info.label.clone(),
                device: f.info.device.clone(),
                kind: SensorKind::Fan,
                value: self.api.rpm(f.gpu, f.index).ok().map(|v| v as f32),
            });
        }
        Ok(())
    }
    fn sensors(&self) -> Vec<SensorInfo> {
        self.sensors.clone()
    }
    fn controls(&self) -> Vec<ControlInfo> {
        self.fans.iter().map(|f| f.info.clone()).collect()
    }
    fn set(&mut self, control: &str, percent: f32) -> Result<()> {
        let i = self
            .fans
            .iter()
            .position(|f| f.info.id == control)
            .context("no NVIDIA fan with this ID")?;
        self.fans[i].info.managed = true;
        // Persist before NVML switches the fan to manual mode. Retry on a failed
        // write, retaining the record until automatic control is restored.
        self.record()?;
        let fan = &self.fans[i];
        // Read temperature directly even for a manual/flat curve, so GPU cooling
        // cannot be held low by a missing sensor or a hot GPU.
        let temp = self.api.temperature(fan.gpu).ok().map(|v| v as f32);
        self.api.set(
            fan.gpu,
            fan.index,
            bounded_duty(percent, fan.info.min_percent, fan.info.max_percent, temp),
        )
    }
    fn release(&mut self, control: &str) -> Result<()> {
        let fan = self
            .fans
            .iter_mut()
            .find(|f| f.info.id == control)
            .context("no NVIDIA fan with this ID")?;
        if fan.info.managed {
            self.api.restore(fan.gpu, fan.index)?;
            fan.info.managed = false;
            self.record()?;
        }
        Ok(())
    }
    fn release_all(&mut self) {
        let ids: Vec<_> = self
            .fans
            .iter()
            .filter(|f| f.info.managed)
            .map(|f| f.info.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.release(&id) {
                warn!("restoring {id}: {e:#}");
            }
        }
    }
}

impl Drop for NvidiaBackend {
    fn drop(&mut self) {
        self.release_all();
    }
}

pub fn restore_path(hwmon_path: &Path) -> PathBuf {
    hwmon_path.with_extension("nvidia.json")
}

/// Restore only the UUID/fan pairs recorded by this daemon, including after SIGKILL.
pub fn restore_from_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut ids: BTreeSet<String> = serde_json::from_slice(&fs::read(path)?)?;
    let mut backend = NvidiaBackend::new(None)?;
    for fan in &mut backend.fans {
        if ids.contains(&fan.info.id) {
            if let Err(e) = backend.api.restore(fan.gpu, fan.index) {
                warn!("restoring {}: {e:#}", fan.info.id);
            } else {
                ids.remove(&fan.info.id);
            }
        }
    }
    write_record(path, &ids)?;
    if !ids.is_empty() {
        bail!(
            "could not restore {} NVIDIA fans; recovery record retained",
            ids.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        writes: Vec<(u32, u32)>,
        restored: Vec<u32>,
        fail_restore: bool,
        hot: bool,
        missing_temp: bool,
    }
    struct Fake(Arc<Mutex<State>>);
    impl NvmlApi for Fake {
        fn devices(&self) -> Result<Vec<Gpu>> {
            Ok(vec![Gpu {
                handle: 1,
                uuid: "GPU-test".into(),
                name: "RTX test".into(),
                fans: 2,
                min: 30,
                max: 100,
            }])
        }
        fn temperature(&self, _: usize) -> Result<u32> {
            let s = self.0.lock().unwrap();
            if s.missing_temp {
                bail!("sensor unavailable");
            }
            Ok(if s.hot { 85 } else { 45 })
        }
        fn percent(&self, _: usize, _: u32) -> Result<u32> {
            Ok(30)
        }
        fn rpm(&self, _: usize, fan: u32) -> Result<u32> {
            Ok(1200 + fan * 10)
        }
        fn set(&self, _: usize, fan: u32, speed: u32) -> Result<()> {
            self.0.lock().unwrap().writes.push((fan, speed));
            Ok(())
        }
        fn restore(&self, _: usize, fan: u32) -> Result<()> {
            let mut s = self.0.lock().unwrap();
            if s.fail_restore {
                bail!("driver unavailable");
            }
            s.restored.push(fan);
            Ok(())
        }
    }
    fn backend(path: Option<PathBuf>) -> (NvidiaBackend, Arc<Mutex<State>>) {
        let state = Arc::new(Mutex::new(State::default()));
        (
            NvidiaBackend::with_api(Box::new(Fake(state.clone())), path).unwrap(),
            state,
        )
    }
    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "airbend-nvidia-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn enumerates_independent_fans_rpm_and_temperature_and_enforces_limits() {
        let (mut b, state) = backend(None);
        b.refresh().unwrap();
        let controls = b.controls();
        assert_eq!(controls.len(), 2);
        assert_eq!(
            controls[1].paired_fan.as_deref(),
            Some("nvidia:GPU-test/rpm2")
        );
        assert!(!controls[0].calibration_supported);
        assert_eq!(b.sensors()[0].value, Some(45.0));
        assert_eq!(b.sensors()[2].value, Some(1210.0));
        b.set(&controls[0].id, 0.0).unwrap();
        b.set(&controls[1].id, 150.0).unwrap();
        assert_eq!(state.lock().unwrap().writes, [(0, 30), (1, 100)]);
    }

    #[test]
    fn hot_or_unreadable_gpu_overrides_manual_duty_immediately() {
        let (mut b, state) = backend(None);
        let id = b.controls()[0].id.clone();
        state.lock().unwrap().hot = true;
        b.set(&id, 30.0).unwrap();
        state.lock().unwrap().hot = false;
        state.lock().unwrap().missing_temp = true;
        b.set(&id, 30.0).unwrap();
        assert_eq!(state.lock().unwrap().writes, [(0, 100), (0, 100)]);
    }

    #[test]
    fn failed_recovery_is_retained_and_successful_retry_restores_both_fans() {
        let dir = directory();
        let path = dir.join("restore.nvidia.json");
        let (mut b, state) = backend(Some(path.clone()));
        let ids: Vec<_> = b.controls().into_iter().map(|c| c.id).collect();
        for id in &ids {
            b.set(id, 40.0).unwrap();
        }
        let recorded: BTreeSet<String> = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(recorded.len(), 2);
        state.lock().unwrap().fail_restore = true;
        assert!(b.release(&ids[0]).is_err());
        assert!(path.exists());
        assert!(b.controls()[0].managed);
        state.lock().unwrap().fail_restore = false;
        b.release_all();
        assert_eq!(state.lock().unwrap().restored, [0, 1]);
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn never_takes_manual_control_without_a_recovery_record() {
        let dir = directory();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("blocked"), "not a directory").unwrap();
        let (mut b, state) = backend(Some(dir.join("blocked/restore.json")));
        assert!(b.set("nvidia:GPU-test/fan1", 40.0).is_err());
        assert!(state.lock().unwrap().writes.is_empty());
        drop(b);
        fs::remove_dir_all(dir).unwrap();
    }
}
