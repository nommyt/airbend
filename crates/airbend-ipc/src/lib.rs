//! The daemon ↔ client protocol: one JSON [`Request`] per line, answered by one JSON
//! [`Response`] per line, over a Unix socket (a named pipe on Windows, later).
//!
//! The socket's file permissions are the access control: root and the daemon's group may
//! connect, nobody else.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use airbend_core::calibrate::Progress;
use airbend_core::engine::CurveValue;
use airbend_core::{Config, ControlInfo, SensorInfo, ProfileId};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Longest accepted line: a config with hundreds of curves fits easily.
pub const MAX_LINE: usize = 1 << 20;

pub fn default_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("AIRBEND_SOCKET") {
        return p.into();
    }
    #[cfg(unix)]
    {
        "/run/airbend/airbend.sock".into()
    }
    #[cfg(not(unix))]
    {
        r"\\.\pipe\airbend".into()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello,
    GetState,
    GetConfig,
    /// Replace the whole config. The daemon validates, saves and applies it.
    SetConfig { config: Config },
    SelectProfile { profile: ProfileId },
    /// Run a calibration on one output. The result lands in the config's `calibrations`.
    Calibrate { control: String },
    CancelCalibration { control: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Hello { protocol: u32, daemon: String, backend: String },
    State { state: Snapshot },
    Config { config: Config, revision: u64 },
    Ok { revision: u64 },
    Error { message: String },
}

/// What the daemon saw and did on its latest tick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub backend: String,
    pub tick: u64,
    /// Bumped whenever the config changes, so clients know to refetch it.
    pub config_revision: u64,
    pub sensors: Vec<SensorInfo>,
    pub controls: Vec<ControlInfo>,
    pub curves: BTreeMap<String, CurveValue>,
    /// Calibrations in progress, by control id.
    #[serde(default)]
    pub calibrating: BTreeMap<String, Progress>,
    /// The last calibration failure per control, cleared when it is calibrated again.
    #[serde(default)]
    pub calibration_errors: BTreeMap<String, String>,
    /// Problems worth showing (failed writes, missing sensors), newest last.
    pub warnings: Vec<String>,
}

/// Reads one line, refusing lines longer than [`MAX_LINE`]. `Ok(None)` at end of stream.
pub fn read_line<R: BufRead>(r: &mut R) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    // A final message without a newline (EOF) is accepted; an overlong one is not.
    if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
        bail!("message longer than {MAX_LINE} bytes");
    }
    Ok(Some(String::from_utf8(buf).context("message is not UTF-8")?))
}

pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()?;
    Ok(())
}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

/// A blocking connection to the daemon.
pub struct Client {
    reader: BufReader<Stream>,
    writer: Stream,
}

impl Client {
    #[cfg(unix)]
    pub fn connect(path: &Path) -> Result<Client> {
        let s = Stream::connect(path).with_context(|| format!("connecting to {}", path.display()))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Client { reader: BufReader::new(s.try_clone()?), writer: s })
    }

    pub fn call(&mut self, req: &Request) -> Result<Response> {
        write_msg(&mut self.writer, req)?;
        let line = read_line(&mut self.reader)?.context("daemon closed the connection")?;
        let resp: Response = serde_json::from_str(&line).context("parsing daemon response")?;
        if let Response::Error { message } = &resp {
            bail!("{message}");
        }
        Ok(resp)
    }

    pub fn state(&mut self) -> Result<Snapshot> {
        match self.call(&Request::GetState)? {
            Response::State { state } => Ok(state),
            r => bail!("unexpected response {r:?}"),
        }
    }

    pub fn config(&mut self) -> Result<(Config, u64)> {
        match self.call(&Request::GetConfig)? {
            Response::Config { config, revision } => Ok((config, revision)),
            r => bail!("unexpected response {r:?}"),
        }
    }

    pub fn calibrate(&mut self, control: &str) -> Result<()> {
        self.call(&Request::Calibrate { control: control.into() }).map(drop)
    }

    pub fn select_profile(&mut self, profile: ProfileId) -> Result<u64> {
        match self.call(&Request::SelectProfile { profile })? {
            Response::Ok { revision } => Ok(revision),
            r => bail!("unexpected response {r:?}"),
        }
    }

    pub fn cancel_calibration(&mut self, control: &str) -> Result<()> {
        self.call(&Request::CancelCalibration { control: control.into() }).map(drop)
    }

    pub fn set_config(&mut self, config: Config) -> Result<u64> {
        match self.call(&Request::SetConfig { config })? {
            Response::Ok { revision } => Ok(revision),
            r => bail!("unexpected response {r:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_as_single_lines() {
        let req = Request::SetConfig { config: Config::default() };
        let mut buf = Vec::new();
        write_msg(&mut buf, &req).unwrap();
        assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), 1);
        let line = read_line(&mut &buf[..]).unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), req);
    }

    #[test]
    fn rejects_oversized_lines() {
        let big = vec![b'a'; MAX_LINE + 10];
        assert!(read_line(&mut &big[..]).is_err());
    }
}
