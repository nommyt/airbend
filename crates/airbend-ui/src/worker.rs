//! The daemon connection, on its own thread: polls state once a second, refetches the config
//! when its revision moves, and sends saves. Every message wakes the UI.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use airbend_core::Config;
use airbend_ipc::{Client, Snapshot};
use rune::Waker;

use crate::Conn;

pub enum Cmd {
    Save(Config),
    Calibrate(String),
    CancelCalibration(String),
}

pub enum Msg {
    Conn(Conn),
    State(Snapshot),
    Config(Config, u64),
    Saved(Config, u64),
    SaveFailed(String),
    Notice(bool, String),
}

pub fn spawn(socket: PathBuf, waker: Waker) -> (Sender<Cmd>, Receiver<Msg>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    thread::Builder::new()
        .name("airbend-daemon".into())
        .spawn(move || {
            let send = |m: Msg| {
                let ok = msg_tx.send(m).is_ok();
                waker();
                ok
            };
            loop {
                let mut client = match Client::connect(&socket) {
                    Ok(c) => c,
                    Err(e) => {
                        if !send(Msg::Conn(Conn::Failed(format!("{e:#}")))) {
                            return;
                        }
                        // Saves while disconnected fail at once rather than hanging.
                        match cmd_rx.recv_timeout(Duration::from_secs(2)) {
                            Ok(Cmd::Save(_)) => {
                                send(Msg::SaveFailed("not connected to airbendd".into()));
                            }
                            Ok(_) => {
                                send(Msg::Notice(false, "not connected to airbendd".into()));
                            }
                            Err(RecvTimeoutError::Disconnected) => return,
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                        continue;
                    }
                };
                send(Msg::Conn(Conn::Connected));
                let mut revision = 0;
                let result: anyhow::Result<()> = (|| loop {
                    let state = client.state()?;
                    if state.config_revision != revision {
                        let (cfg, rev) = client.config()?;
                        revision = rev;
                        send(Msg::Config(cfg, rev));
                    }
                    send(Msg::State(state));
                    match cmd_rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(Cmd::Save(cfg)) => match client.set_config(cfg) {
                            Ok(_) => {
                                let (cfg, rev) = client.config()?;
                                revision = rev;
                                send(Msg::Saved(cfg, rev));
                            }
                            Err(e) => {
                                send(Msg::SaveFailed(format!("{e:#}")));
                            }
                        },
                        Ok(Cmd::Calibrate(id)) => match client.calibrate(&id) {
                            Ok(()) => {
                                send(Msg::Notice(true, "Calibration started".into()));
                            }
                            Err(e) => {
                                send(Msg::Notice(false, format!("{e:#}")));
                            }
                        },
                        Ok(Cmd::CancelCalibration(id)) => {
                            if let Err(e) = client.cancel_calibration(&id) {
                                send(Msg::Notice(false, format!("{e:#}")));
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                })();
                match result {
                    Ok(()) => return,
                    Err(e) => {
                        if !send(Msg::Conn(Conn::Failed(format!("{e:#}")))) {
                            return;
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })
        .expect("spawning the daemon thread");
    (cmd_tx, msg_rx)
}
