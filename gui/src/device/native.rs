//! Worker thread owning the PPK2: handles the UI commands and feeds the samples into the [Live] buffer and, while
//! recording, the [Recording]. It measures as long as it is connected.
//!
//! [Live]: crate::recording::Live

use std::{
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use ppk2::{
    measurement::MeasurementMatch,
    try_find_ppk2_port,
    types::{DevicePower, MeasurementMode, SourceVoltage},
    Ppk2,
};

use super::{Cmd, Connection, Shared, LIVE_SECONDS};
use crate::recording::{Live, Recording};

/// Handle to the worker thread
pub struct Device {
    tx: Sender<Cmd>,
    pub shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<()>>,
}

impl Device {
    pub fn spawn(ctx: eframe::egui::Context, recording: Recording) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            connection: None,
            measuring: false,
            streaming: false,
            live: Live::new(LIVE_SECONDS),
            error: None,
            recording,
        }));
        let (tx, rx) = mpsc::channel();
        let mut worker = Worker {
            rx,
            shared: shared.clone(),
            ctx,
            state: State::Disconnected,
            voltage: SourceVoltage::from_millivolts(3000),
            power: DevicePower::Disabled,
            recording: false,
            batch: Vec::new(),
        };
        let thread = thread::Builder::new()
            .name("ppk2".into())
            .spawn(move || worker.run())
            .expect("Failed to spawn the PPK2 thread");
        Self { tx, shared, thread: Some(thread) }
    }

    pub fn send(&self, cmd: Cmd) {
        // Only fails if the worker is gone, which the UI notices from the shared state
        let _ = self.tx.send(cmd);
    }
}

impl Device {
    /// Stop the worker, which stops a running measurement. Waits for it a few seconds at most, so a device that
    /// doesn't respond can't keep the app from exiting.
    pub fn shutdown(&mut self) {
        let Some(t) = self.thread.take() else { return };
        self.send(Cmd::Shutdown);
        let start = Instant::now();
        while !t.is_finished() && start.elapsed() < Duration::from_secs(3) {
            thread::sleep(Duration::from_millis(10));
        }
        if t.is_finished() {
            let _ = t.join();
        } else {
            tracing::warn!("The PPK2 thread didn't stop, exiting anyway");
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type StopFn = Box<dyn FnOnce() -> ppk2::Result<Ppk2> + Send>;

enum State {
    Disconnected,
    Idle(Ppk2),
    Measuring {
        rx: Receiver<MeasurementMatch>,
        stop: StopFn,
        /// Second handle to send commands while measuring
        ctl: Ppk2,
    },
}

struct Worker {
    rx: Receiver<Cmd>,
    shared: Arc<Mutex<Shared>>,
    ctx: eframe::egui::Context,
    state: State,
    voltage: SourceVoltage,
    power: DevicePower,
    /// Whether the samples go into the recording
    recording: bool,
    /// Received samples: current (µA) and logic port pins
    batch: Vec<(f32, u8)>,
}

impl Worker {
    fn run(&mut self) {
        loop {
            let cmd = if matches!(self.state, State::Measuring { .. }) {
                self.pump();
                match self.rx.try_recv() {
                    Ok(cmd) => cmd,
                    Err(mpsc::TryRecvError::Empty) => continue,
                    Err(mpsc::TryRecvError::Disconnected) => Cmd::Shutdown,
                }
            } else {
                self.rx.recv().unwrap_or(Cmd::Shutdown)
            };

            if matches!(cmd, Cmd::Shutdown) {
                if let State::Measuring { stop, .. } = std::mem::replace(&mut self.state, State::Disconnected) {
                    let _ = stop();
                }
                return;
            }

            self.shared.lock().unwrap().error = None;
            // Any error leaves the device disconnected
            let state = std::mem::replace(&mut self.state, State::Disconnected);
            match self.handle(state, cmd) {
                Ok(state) => self.state = state,
                Err(e) => {
                    tracing::error!("{e}");
                    self.shared.lock().unwrap().error = Some(e.to_string());
                }
            }
            self.publish();
        }
    }

    /// Move the received samples into the live buffer and, while recording, the recording
    fn pump(&mut self) {
        let State::Measuring { rx, .. } = &self.state else { return };
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(first) => {
                self.batch.clear();
                for m in std::iter::once(first).chain(rx.try_iter().take(100_000)) {
                    if let MeasurementMatch::Match(m) = m {
                        let pins = (0..8).filter(|&i| m.pins.pin_is_high(i)).fold(0u8, |acc, i| acc | 1 << i);
                        self.batch.push((m.micro_amps, pins));
                    }
                }
                let mut shared = self.shared.lock().unwrap();
                for &(v, pins) in &self.batch {
                    shared.live.push(v);
                    if self.recording {
                        shared.recording.push(v, pins);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // The measurement thread stopped, e.g. because the PPK2 was unplugged
                let State::Measuring { stop, .. } = std::mem::replace(&mut self.state, State::Disconnected) else {
                    unreachable!()
                };
                let error = match stop() {
                    Err(e) => e.to_string(),
                    Ok(_) => "Measurement stopped unexpectedly".into(),
                };
                tracing::error!("{error}");
                self.shared.lock().unwrap().error = Some(error);
                self.publish();
            }
        }
    }

    fn handle(&mut self, state: State, cmd: Cmd) -> ppk2::Result<State> {
        Ok(match (state, cmd) {
            (state, Cmd::Connect { port, mode }) => {
                drop(stop_measuring(state)?);
                let port = match port {
                    Some(p) => p,
                    None => try_find_ppk2_port()?,
                };
                let mut ppk2 = Ppk2::new(port.clone(), mode)?;
                ppk2.set_source_voltage(self.voltage)?;
                ppk2.set_device_power(self.power)?;
                let meta = ppk2.metadata();
                let mut shared = self.shared.lock().unwrap();
                shared.connection = Some(Connection {
                    port,
                    mode,
                    hw: meta.hw,
                    calibrated: meta.calibrated,
                    has_calibration_values: meta.has_calibration_values(),
                });
                shared.recording.source_mv = (mode == MeasurementMode::Source).then(|| self.voltage.millivolts());
                shared.live.clear();
                drop(shared);
                // Measure right away, for the live readout
                self.recording = false;
                start_measuring(ppk2)?
            }
            (state, Cmd::Disconnect) => {
                drop(stop_measuring(state)?);
                self.shared.lock().unwrap().connection = None;
                self.recording = false;
                State::Disconnected
            }
            (state, Cmd::SetVoltage(mv)) => {
                self.voltage = SourceVoltage::from_millivolts(mv);
                // Restart the measurement, as the current calculation depends on the source voltage
                let was_measuring = matches!(state, State::Measuring { .. });
                match stop_measuring(state)? {
                    Some(mut ppk2) => {
                        ppk2.set_source_voltage(self.voltage)?;
                        let mut shared = self.shared.lock().unwrap();
                        if shared.recording.source_mv.is_some() {
                            shared.recording.source_mv = Some(self.voltage.millivolts());
                        }
                        drop(shared);
                        if was_measuring {
                            start_measuring(ppk2)?
                        } else {
                            State::Idle(ppk2)
                        }
                    }
                    None => State::Disconnected,
                }
            }
            (state, Cmd::SetPower(on)) => {
                self.power = if on { DevicePower::Enabled } else { DevicePower::Disabled };
                match state {
                    State::Disconnected => State::Disconnected,
                    State::Idle(mut ppk2) => {
                        ppk2.set_device_power(self.power)?;
                        State::Idle(ppk2)
                    }
                    // Without interrupting the measurement, to see e.g. the inrush current
                    State::Measuring { rx, stop, mut ctl } => {
                        if let Err(e) = ctl.set_device_power(self.power) {
                            let _ = stop();
                            return Err(e);
                        }
                        State::Measuring { rx, stop, ctl }
                    }
                }
            }
            (state, Cmd::Start) => {
                let state = match state {
                    State::Idle(ppk2) => start_measuring(ppk2)?,
                    state => state,
                };
                if matches!(state, State::Measuring { .. }) {
                    self.shared.lock().unwrap().recording.resume();
                    self.recording = true;
                }
                state
            }
            (state, Cmd::Stop) => {
                self.recording = false;
                state
            }
            (_, Cmd::Shutdown) => unreachable!(),
        })
    }

    /// Update the shared state from the worker state
    fn publish(&mut self) {
        let mut shared = self.shared.lock().unwrap();
        shared.streaming = matches!(self.state, State::Measuring { .. });
        shared.measuring = shared.streaming && self.recording;
        if !shared.streaming {
            self.recording = false;
        }
        if matches!(self.state, State::Disconnected) {
            shared.connection = None;
        }
        drop(shared);
        self.ctx.request_repaint();
    }
}

/// Stop measuring if needed, returning the device if connected
fn stop_measuring(state: State) -> ppk2::Result<Option<Ppk2>> {
    Ok(match state {
        State::Disconnected => None,
        State::Idle(ppk2) => Some(ppk2),
        State::Measuring { stop, .. } => Some(stop()?),
    })
}

fn start_measuring(ppk2: Ppk2) -> ppk2::Result<State> {
    let ctl = ppk2.try_clone()?;
    let (rx, stop) = ppk2.start_measurement(100_000)?;
    Ok(State::Measuring { rx, stop: Box::new(stop), ctl })
}
