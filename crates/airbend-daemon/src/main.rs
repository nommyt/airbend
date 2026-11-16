//! airbendd: the privileged half of airbend.
//!
//! The main thread owns the hardware backend and runs the control loop. A listener thread
//! serves clients over a Unix socket; clients read state and replace the config, and never
//! touch hardware directly.
//!
//! Shutdown (SIGINT/SIGTERM, or a panic unwinding through the loop) hands every output back
//! to the firmware. For harder deaths (SIGKILL, OOM), systemd runs `airbendd --restore`, which
//! replays the restore file written when outputs were taken.

use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, thread};

use airbend_core::calibrate::{Calibrator, Step};
use airbend_core::engine::ControlTarget;
use airbend_core::{Config, Engine, SensorKind};
use airbend_hw::Backend;
use airbend_ipc::{PROTOCOL_VERSION, Request, Response, Snapshot};
use anyhow::{Context, Result, bail};
use log::{error, info, warn};

const MAX_WARNINGS: usize = 20;

struct Args {
    config: PathBuf,
    socket: PathBuf,
    restore_file: PathBuf,
    group: String,
    mock: bool,
    restore: bool,
}

fn usage() -> ! {
    eprintln!(
        "airbendd {}: fan control daemon

USAGE: airbendd [--config PATH] [--socket PATH] [--group NAME] [--mock]
       airbendd --restore

  --config PATH   config file (default /etc/airbend/config.toml)
  --socket PATH   control socket (default /run/airbend/airbend.sock, or $AIRBEND_SOCKET)
  --group NAME    group allowed to use the socket (default wheel)
  --restore-file  where takeovers are recorded (default /run/airbend/restore.json)
  --mock          simulated hardware, for development; needs no root
  --restore       hand outputs left by a dead daemon back to the firmware, then exit",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        config: "/etc/airbend/config.toml".into(),
        socket: airbend_ipc::default_socket(),
        restore_file: "/run/airbend/restore.json".into(),
        group: "wheel".into(),
        mock: false,
        restore: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--config" => a.config = val().into(),
            "--socket" => a.socket = val().into(),
            "--group" => a.group = val(),
            "--restore-file" => a.restore_file = val().into(),
            "--mock" => a.mock = true,
            "--restore" => a.restore = true,
            "-h" | "--help" => usage(),
            "-V" | "--version" => {
                println!("airbendd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0)
            }
            _ => {
                eprintln!("unknown argument {arg}");
                usage()
            }
        }
    }
    a
}

/// State shared between the control loop and client threads.
struct Shared {
    config: Config,
    revision: u64,
    snapshot: Snapshot,
    config_path: PathBuf,
    /// Calibration starts and cancels from clients, for the loop to pick up.
    calibration_requests: Vec<CalibrationRequest>,
    active_calibration: Option<String>,
}

enum CalibrationRequest {
    Start(String),
    Cancel(String),
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args();
    if let Err(e) = run(args) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if args.restore {
        #[cfg(target_os = "linux")]
        {
            // Attempt both recoveries even when one device/driver is unavailable.
            let hw = airbend_hw::linux::restore_from_file(&args.restore_file);
            let gpu = airbend_hw::nvidia::restore_from_file(&airbend_hw::nvidia::restore_path(&args.restore_file));
            let n = hw?;
            gpu?;
            info!("restored {n} outputs");
        }
        return Ok(());
    }

    let config = load_config(&args.config)?;
    let opts = airbend_hw::Options {
        mock: args.mock,
        restore_file: (!args.mock).then(|| args.restore_file.clone()),
    };
    let mut backend = airbend_hw::open(&opts)?;
    backend.refresh()?;

    let shared = Arc::new(Mutex::new(Shared {
        config: config.clone(),
        revision: 1,
        snapshot: Snapshot { backend: backend.name().into(), ..Snapshot::default() },
        config_path: args.config.clone(),
        calibration_requests: Vec::new(),
        active_calibration: None,
    }));

    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, stop.clone())?;
    }

    let listener = bind_socket(&args.socket, &args.group)?;
    {
        let shared = shared.clone();
        thread::Builder::new().name("ipc".into()).spawn(move || serve(listener, shared))?;
    }

    info!("running with the {} backend, config {}", backend.name(), args.config.display());
    let result = control_loop(backend.as_mut(), config, &shared, &stop);
    info!("shutting down; releasing outputs");
    backend.release_all();
    let _ = fs::remove_file(&args.socket);
    result
}

fn load_config(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(s) => Config::from_toml(&s).with_context(|| format!("loading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("no config at {}; starting with every output left to the firmware", path.display());
            Ok(Config::default())
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn save_config(path: &Path, config: &Config) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, config.to_toml()?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn control_loop(backend: &mut dyn Backend, config: Config, shared: &Mutex<Shared>, stop: &AtomicBool) -> Result<()> {
    let mut engine = Engine::new(config);
    let mut revision = 1;
    let mut last = Instant::now();
    let mut tick: u64 = 0;
    let mut warnings: Vec<String> = Vec::new();
    let clock = Instant::now();
    let mut calibrators: BTreeMap<String, Calibrator> = BTreeMap::new();
    let mut calibration_errors: BTreeMap<String, String> = BTreeMap::new();
    let warn_once = |warnings: &mut Vec<String>, w: String| {
        if warnings.last() != Some(&w) {
            warn!("{w}");
            warnings.push(w);
            if warnings.len() > MAX_WARNINGS {
                warnings.remove(0);
            }
        }
    };

    while !stop.load(Ordering::Relaxed) {
        // Pick up a new config and calibration requests.
        let requests = {
            let mut s = shared.lock().unwrap();
            if s.revision != revision {
                revision = s.revision;
                engine.set_config(s.config.clone());
                info!("applied config revision {revision}");
            }
            std::mem::take(&mut s.calibration_requests)
        };
        let now_s = clock.elapsed().as_secs_f64();
        for r in requests {
            match r {
                CalibrationRequest::Start(id) => {
                    let hint = backend.controls().into_iter().find(|c| c.id == id).and_then(|c| c.paired_fan);
                    info!("calibrating {id}");
                    calibration_errors.remove(&id);
                    calibrators.insert(id, Calibrator::new(now_s, hint));
                }
                CalibrationRequest::Cancel(id) => {
                    if calibrators.remove(&id).is_some() {
                        shared.lock().unwrap().active_calibration = None;
                        info!("calibration of {id} cancelled");
                        calibration_errors.insert(id, "cancelled".into());
                    }
                }
            }
        }

        if let Err(e) = backend.refresh() {
            warn_once(&mut warnings, format!("reading sensors: {e:#}"));
        }
        let sensors = backend.sensors();
        let temps: HashMap<String, f32> = sensors
            .iter()
            .filter(|s| s.kind == SensorKind::Temperature)
            .filter_map(|s| Some((s.id.clone(), s.value?)))
            .collect();

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(5.0);
        last = now;
        let out = engine.tick(&temps, if tick == 0 { 0.0 } else { dt });

        for (id, v) in &out.curves {
            if let Some(e) = &v.error {
                warn_once(&mut warnings, format!("curve {id}: {e}; failsafe 100 %"));
            }
        }

        // Calibrations override their output's target until they finish.
        let fans: BTreeMap<String, f32> = sensors
            .iter()
            .filter(|s| s.kind == SensorKind::Fan)
            .filter_map(|s| Some((s.id.clone(), s.value?)))
            .collect();
        let temperature_missing = sensors.iter().any(|s| {
            s.kind == SensorKind::Temperature && s.value.is_none_or(|v| !v.is_finite())
        });
        let hottest = if temperature_missing { None } else { temps.values().copied().reduce(f32::max) };
        let mut calibration_duty: BTreeMap<String, f32> = BTreeMap::new();
        let mut finished = Vec::new();
        for (id, c) in calibrators.iter_mut() {
            // A changing GPU fan must not be mistaken for the motherboard fan
            // being calibrated. Tachometers belong to the output's controller.
            match c.step(now_s, &calibration_fans(id, &fans), hottest) {
                Step::Run(d) => {
                    calibration_duty.insert(id.clone(), d);
                }
                Step::Done(result) => finished.push((id.clone(), Ok(result))),
                Step::Failed(e) => finished.push((id.clone(), Err(e))),
            }
        }
        for (id, result) in finished {
            calibrators.remove(&id);
            shared.lock().unwrap().active_calibration = None;
            match result {
                Ok(cal) => {
                    info!("calibrated {id}: {cal:?}");
                    let mut s = shared.lock().unwrap();
                    let mut cfg = s.config.clone();
                    cfg.calibrations.insert(id.clone(), cal);
                    match save_config(&s.config_path, &cfg) {
                        Ok(()) => {
                            s.config = cfg;
                            s.revision += 1;
                        }
                        Err(e) => {
                            calibration_errors.insert(id, format!("saving the result: {e:#}"));
                        }
                    }
                }
                Err(e) => {
                    warn!("calibration of {id} failed: {e}");
                    calibration_errors.insert(id, e);
                }
            }
        }

        let hw_controls = backend.controls();
        for c in &hw_controls {
            let target = match calibration_duty.get(&c.id) {
                Some(d) => Some(&ControlTarget::Set(*d)),
                None => out.controls.get(&c.id),
            };
            let r = match target {
                Some(ControlTarget::Set(p)) => backend.set(&c.id, *p),
                // Not configured, disabled, or no curve: leave it to the firmware.
                _ => backend.release(&c.id),
            };
            if let Err(e) = r {
                warn_once(&mut warnings, format!("{}: {e:#}", c.id));
                if calibrators.remove(&c.id).is_some() {
                    shared.lock().unwrap().active_calibration = None;
                    calibration_errors.insert(c.id.clone(), format!("stopped: output write failed: {e:#}"));
                    if let Err(e) = backend.release(&c.id) {
                        warn_once(&mut warnings, format!("restoring {}: {e:#}", c.id));
                    }
                }
            }
        }
        for id in out.controls.keys() {
            if !hw_controls.iter().any(|c| &c.id == id) {
                warn_once(&mut warnings, format!("configured control {id} is not present"));
            }
        }

        // Re-read so the snapshot shows what we just wrote.
        let _ = backend.refresh();
        tick += 1;
        {
            let mut s = shared.lock().unwrap();
            s.snapshot = Snapshot {
                backend: backend.name().into(),
                tick,
                config_revision: s.revision,
                sensors: backend.sensors(),
                controls: backend.controls(),
                curves: out.curves,
                calibrating: calibrators.iter().map(|(id, c)| (id.clone(), c.progress())).collect(),
                calibration_errors: calibration_errors.clone(),
                warnings: warnings.clone(),
            };
        }

        // Sleep until the next tick in small slices so signals are noticed quickly.
        let period = Duration::from_millis(engine.config().tick_ms);
        while !stop.load(Ordering::Relaxed) && last.elapsed() < period {
            thread::sleep(Duration::from_millis(50).min(period.saturating_sub(last.elapsed())));
            if shared.lock().unwrap().revision != revision {
                break;
            }
        }
    }
    Ok(())
}

fn calibration_fans(control: &str, fans: &BTreeMap<String, f32>) -> BTreeMap<String, f32> {
    let prefix = control.rsplit_once('/').map(|(device, _)| format!("{device}/"));
    fans.iter().filter(|(id, _)| prefix.as_ref().is_none_or(|p| id.starts_with(p)))
        .map(|(id, rpm)| (id.clone(), *rpm)).collect()
}

#[cfg(unix)]
fn bind_socket(path: &Path, group: &str) -> Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("another airbendd is already listening on {}", path.display());
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    let is_root = unsafe { libc::geteuid() } == 0;
    if is_root {
        match group_id(group) {
            Some(gid) => {
                let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
                if unsafe { libc::chown(c.as_ptr(), 0, gid) } != 0 {
                    bail!("chown {}: {}", path.display(), std::io::Error::last_os_error());
                }
                fs::set_permissions(path, fs::Permissions::from_mode(0o660))?;
            }
            None => {
                warn!("group {group:?} not found; only root can use the socket");
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
        }
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    info!("listening on {}", path.display());
    Ok(listener)
}

#[cfg(unix)]
fn group_id(name: &str) -> Option<libc::gid_t> {
    let c = std::ffi::CString::new(name).ok()?;
    let g = unsafe { libc::getgrnam(c.as_ptr()) };
    (!g.is_null()).then(|| unsafe { (*g).gr_gid })
}

#[cfg(unix)]
fn serve(listener: std::os::unix::net::UnixListener, shared: Arc<Mutex<Shared>>) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let shared = shared.clone();
        let _ = thread::Builder::new().name("client".into()).spawn(move || {
            if let Err(e) = handle_client(stream, &shared) {
                log::debug!("client: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
fn handle_client(stream: std::os::unix::net::UnixStream, shared: &Mutex<Shared>) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    while let Some(line) = airbend_ipc::read_line(&mut reader)? {
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, shared),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };
        airbend_ipc::write_msg(&mut writer, &resp)?;
    }
    Ok(())
}

fn handle(req: Request, shared: &Mutex<Shared>) -> Response {
    match req {
        Request::Hello => Response::Hello {
            protocol: PROTOCOL_VERSION,
            daemon: env!("CARGO_PKG_VERSION").into(),
            backend: shared.lock().unwrap().snapshot.backend.clone(),
        },
        Request::GetState => Response::State { state: shared.lock().unwrap().snapshot.clone() },
        Request::GetConfig => {
            let s = shared.lock().unwrap();
            Response::Config { config: s.config.clone(), revision: s.revision }
        }
        Request::SetConfig { mut config } => {
            if let Err(e) = config.validate() {
                return Response::Error { message: format!("invalid config: {e:#}") };
            }
            let mut s = shared.lock().unwrap();
            // Results are daemon-owned. A client may still be editing the draft from
            // before a calibration completed, or may not have fetched it yet.
            config.calibrations = s.config.calibrations.clone();
            if let Err(e) = save_config(&s.config_path, &config) {
                return Response::Error { message: format!("saving config: {e:#}") };
            }
            s.config = config;
            s.revision += 1;
            info!("config updated to revision {}", s.revision);
            Response::Ok { revision: s.revision }
        }
        Request::Calibrate { control } => {
            let mut s = shared.lock().unwrap();
            if s.snapshot.controls.iter().any(|c| c.id == control && !c.calibration_supported) {
                return Response::Error { message: "this fan uses the driver's speed limits; stop/start calibration is unavailable".into() };
            }
            if !s.snapshot.controls.iter().any(|c| c.id == control) {
                return Response::Error { message: format!("no control {control}") };
            }
            if s.active_calibration.is_some() {
                // One at a time: the fan is identified by which tachometer moves.
                return Response::Error { message: "another fan is being calibrated".into() };
            }
            s.active_calibration = Some(control.clone());
            s.calibration_requests.push(CalibrationRequest::Start(control));
            Response::Ok { revision: s.revision }
        }
        Request::CancelCalibration { control } => {
            let mut s = shared.lock().unwrap();
            s.calibration_requests.push(CalibrationRequest::Cancel(control));
            Response::Ok { revision: s.revision }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airbend_core::{ControlInfo, calibrate::Calibration};

    #[test]
    fn a_changing_gpu_fan_cannot_steal_motherboard_calibration_pairing() {
        let mut c = Calibrator::new(0.0, Some("hwmon:board/fan1".into()));
        let mut duty = 100.0;
        for t in 1..=200 {
            let fans = [("hwmon:board/fan1".into(), duty * 18.0),
                ("nvidia:GPU-test/rpm1".into(), if t <= 6 { 5000.0 } else { 0.0 })].into_iter().collect();
            match c.step(t as f64, &calibration_fans("hwmon:board/pwm1", &fans), Some(50.0)) {
                Step::Run(d) => duty = d,
                Step::Done(result) => { assert_eq!(result.fan.as_deref(), Some("hwmon:board/fan1")); return; }
                Step::Failed(e) => panic!("{e}"),
            }
        }
        panic!("calibration did not finish");
    }

    #[test]
    fn stale_client_cannot_erase_results_or_start_two_calibrations() {
        let dir = std::env::temp_dir().join(format!("airbend-daemon-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.calibrations.insert("pwm1".into(), Calibration {
            fan: Some("fan5".into()), max_rpm: Some(1500.0), stop_percent: Some(25.0),
            start_percent: Some(35.0), curve: Vec::new(), at: 1,
        });
        let shared = Mutex::new(Shared {
            config: cfg.clone(), revision: 1, config_path: path.clone(),
            snapshot: Snapshot { controls: vec![ControlInfo {
                id: "pwm1".into(), label: "Fan".into(), device: "test".into(),
                percent: None, paired_fan: None, managed: false,
                min_percent: 0.0, max_percent: 100.0, calibration_supported: true,
            }], ..Snapshot::default() },
            calibration_requests: Vec::new(), active_calibration: None,
        });
        let mut stale = Config::default();
        stale.labels.insert("pwm1".into(), "Exhaust fans".into());
        assert!(matches!(handle(Request::SetConfig { config: stale }, &shared), Response::Ok { .. }));
        let saved = load_config(&path).unwrap();
        assert_eq!(saved.calibrations, cfg.calibrations);
        assert_eq!(saved.labels["pwm1"], "Exhaust fans");
        assert!(matches!(handle(Request::Calibrate { control: "pwm1".into() }, &shared), Response::Ok { .. }));
        // Even after the control loop drains requests, the next start must be rejected.
        shared.lock().unwrap().calibration_requests.clear();
        assert!(matches!(handle(Request::Calibrate { control: "pwm1".into() }, &shared), Response::Error { .. }));
        {
            let mut s = shared.lock().unwrap();
            s.active_calibration = None;
            s.snapshot.controls[0].calibration_supported = false;
        }
        assert!(matches!(handle(Request::Calibrate { control: "pwm1".into() }, &shared),
            Response::Error { message } if message.contains("driver's speed limits")));
        assert!(shared.lock().unwrap().calibration_requests.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
