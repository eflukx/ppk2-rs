//! The PPK2 connection: a worker that owns the device, handles the UI commands and feeds the samples into the
//! [Shared] state. Natively a thread with the blocking serial port, in the browser an async task with WebSerial.

use ppk2::types::MeasurementMode;

use crate::recording::{Live, Recording};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Device;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{Device, CHOOSE_PORT};

/// Seconds of samples in the live buffer: the longest readout averaging window
const LIVE_SECONDS: f64 = 10.;

/// Commands from the UI to the worker
pub enum Cmd {
    /// Connect to the given port, or find the PPK2 if `None`
    Connect { port: Option<String>, mode: MeasurementMode },
    Disconnect,
    SetVoltage(u16),
    SetPower(bool),
    /// Start or continue the recording
    Start,
    /// Stop the recording; the measurement continues for the live readout
    Stop,
    Shutdown,
}

/// The connected device
#[derive(Clone)]
pub struct Connection {
    pub port: String,
    pub mode: MeasurementMode,
    pub hw: u32,
    /// The "calibrated" flag the device reports
    pub calibrated: bool,
    /// Whether the device has (factory) calibration values
    pub has_calibration_values: bool,
}

/// State shared between the worker and the UI
pub struct Shared {
    pub connection: Option<Connection>,
    /// Whether the samples go into the recording
    pub measuring: bool,
    /// Whether samples arrive (into [Shared::live])
    pub streaming: bool,
    pub live: Live,
    /// The last error, until the next command
    pub error: Option<String>,
    pub recording: Recording,
}

