use std::path::PathBuf;
use airbend_core::{Config, ProfileId, SensorKind};
use airbend_ipc::{Client, Snapshot};
use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Serialize)]
struct Fan { label: String, rpm: Option<f32>, percent: Option<f32>, managed: bool }
#[derive(Serialize)]
struct Temperature { label: String, value: f32 }
#[derive(Serialize)]
struct Status {
    connected: bool,
    profile: ProfileId,
    calibrating: bool,
    fans: Vec<Fan>,
    temperatures: Vec<Temperature>,
    warnings: Vec<String>,
}

fn bounded(s: &str) -> String { s.chars().filter(|c| !c.is_control()).take(160).collect() }

fn status(state: Snapshot, config: Config) -> Status {
    let mut fans = Vec::new();
    for c in state.controls {
        let paired = config.calibrations.get(&c.id).and_then(|cal| cal.fan.as_ref()).or(c.paired_fan.as_ref());
        let rpm = paired.and_then(|id| state.sensors.iter().find(|s| &s.id == id)).and_then(|s| s.value).filter(|v| v.is_finite());
        let known = c.managed || c.id.starts_with("nvidia:") || rpm.is_some_and(|v| v > 0.0)
            || config.control(&c.id).is_some_and(|c| c.enabled)
            || config.calibrations.get(&c.id).is_some_and(|cal| cal.fan.is_some());
        if known && fans.len() < 32 {
            fans.push(Fan { label: bounded(config.labels.get(&c.id).unwrap_or(&c.label)), rpm,
                percent: c.percent.filter(|v| v.is_finite()), managed: c.managed });
        }
    }
    // A tray overview should not bury the GPU behind duplicate board sensors.
    // Show the hottest reading for CPU, GPU and SSD; keep the iGPU separate.
    let has_nvidia = state.sensors.iter().any(|s| s.id.starts_with("nvidia:"));
    let mut groups = std::collections::BTreeMap::<u8, Temperature>::new();
    for s in state.sensors.iter().filter(|s| s.kind == SensorKind::Temperature) {
        let Some(value) = s.value.filter(|v| v.is_finite()) else { continue };
        let group = if s.device == "k10temp" || s.device == "coretemp" || s.device.contains("cpu_thermal") { 0 }
            else if s.id.starts_with("nvidia:") { 1 }
            else if s.device == "nvme" { 2 }
            else if s.device == "amdgpu" { if has_nvidia { 3 } else { 1 } }
            else { continue };
        if groups.get(&group).is_none_or(|t| value > t.value) {
            groups.insert(group, Temperature { label: bounded(config.labels.get(&s.id).unwrap_or(&s.label)), value });
        }
    }
    let temperatures = if groups.is_empty() {
        state.sensors.iter().filter(|s| s.kind == SensorKind::Temperature)
            .filter_map(|s| Some(Temperature { label: bounded(config.labels.get(&s.id).unwrap_or(&s.label)), value: s.value.filter(|v| v.is_finite())? }))
            .take(8).collect()
    } else { groups.into_values().collect() };
    Status { connected: true, profile: config.active_profile, calibrating: !state.calibrating.is_empty(),
        fans, temperatures, warnings: state.warnings.iter().rev().take(3).map(|s| bounded(s)).collect() }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut socket = airbend_ipc::default_socket();
    if args.first().map(String::as_str) == Some("--socket") {
        socket = PathBuf::from(args.get(1).context("--socket requires a path")?);
        args.drain(..2);
    }
    if args.first().is_none_or(|a| a == "--help" || a == "-h") {
        println!("airbendctl [--socket PATH] status [--json]\nairbendctl [--socket PATH] profile [quiet|performance]");
        return Ok(());
    }
    let mut client = Client::connect(&socket)?;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["status"] | ["status", "--json"] => {
            let state = client.state()?;
            let (config, _) = client.config()?;
            println!("{}", serde_json::to_string(&status(state, config))?);
        }
        ["profile"] => println!("{}", client.config()?.0.active_profile.id()),
        ["profile", name] => {
            let profile = ProfileId::parse(name)?;
            client.select_profile(profile)?;
            println!("{}", profile.id());
        }
        _ => bail!("unknown command; use airbendctl --help"),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airbend_core::{ControlInfo, SensorInfo};

    #[test]
    fn compact_status_keeps_known_gpu_fans_and_omits_empty_headers() {
        let make = |id: &str, paired| ControlInfo { id: id.into(), label: "<img src='remote'>".into(), device: "test".into(),
            percent: Some(30.0), paired_fan: paired, managed: false, min_percent: 0.0, max_percent: 100.0, calibration_supported: true };
        let s = Snapshot { controls: vec![make("board/pwm2", Some("board/fan2".into())), make("nvidia:GPU/fan1", None)],
            sensors: vec![SensorInfo { id: "board/fan2".into(), label: "unused".into(), device: "test".into(),
                kind: SensorKind::Fan, value: Some(0.0) }], ..Snapshot::default() };
        let view = status(s, Config::default());
        assert_eq!(view.fans.len(), 1);
        assert_eq!(view.fans[0].label, "<img src='remote'>"); // QML must render this as plain text.
        assert!(view.fans[0].rpm.is_none());
    }
}
