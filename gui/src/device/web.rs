//! Async task owning the PPK2 over WebSerial (Chrome, Edge): handles the UI commands and feeds the samples into the
//! [Live] buffer and, while recording, the [Recording]. It measures as long as it is connected.
//!
//! The same protocol as the native [ppk2::Ppk2]: the commands, metadata and sample decoding come from the `ppk2`
//! crate, only the port is different.
//!
//! [Live]: crate::recording::Live

use std::{
    collections::VecDeque,
    pin::pin,
    sync::{Arc, Mutex},
};

use futures::{
    channel::mpsc::{self, UnboundedReceiver, UnboundedSender},
    future::{select, Either},
    StreamExt,
};
use gloo_timers::future::TimeoutFuture;
use js_sys::{futures::JsFuture, Reflect, Uint8Array};
use ppk2::{
    cmd::Command,
    measurement::{Measurement, MeasurementAccumulator},
    types::{DevicePower, MeasurementMode, Metadata, SourceVoltage},
    PPK2_PID, PPK2_VID,
};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    ReadableStreamDefaultReader, SerialOptions, SerialOutputSignals, SerialPort, SerialPortFilter,
    SerialPortRequestOptions, WritableStreamDefaultWriter,
};
use web_time::{Duration, Instant};

use super::{Cmd, Connection, Shared, LIVE_SECONDS};
use crate::recording::{Live, Recording};

/// The [Cmd::Connect] port that asks the user to choose a port, instead of using one allowed before
pub const CHOOSE_PORT: &str = "choose";
/// Maximum time to wait for the complete response to a command
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum time to wait for a running measurement to stop
const STOP_STREAMING_TIMEOUT: Duration = Duration::from_secs(3);
/// Size of the browser's receive buffer (bytes): the PPK2 sends 400 kB/s, and the page may not run for a while,
/// e.g. while a dialog is open
const BUFFER_SIZE: u32 = 4 << 20;

type Result<T> = std::result::Result<T, String>;

/// Handle to the worker task
pub struct Device {
    tx: UnboundedSender<Cmd>,
    pub shared: Arc<Mutex<Shared>>,
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
        let (tx, rx) = mpsc::unbounded();
        let worker = Worker {
            rx,
            shared: shared.clone(),
            ctx,
            port: None,
            voltage: SourceVoltage::from_millivolts(3000),
            power: DevicePower::Disabled,
            recording: false,
            samples: VecDeque::new(),
        };
        wasm_bindgen_futures::spawn_local(worker.run());
        Self { tx, shared }
    }

    pub fn send(&self, cmd: Cmd) {
        // Only fails if the worker is gone, which the UI notices from the shared state
        let _ = self.tx.unbounded_send(cmd);
    }

    /// Stop the worker, which stops a running measurement. Doesn't wait for it: a page can't block.
    pub fn shutdown(&mut self) {
        self.send(Cmd::Shutdown);
    }
}

/// An open port with a running measurement
struct Port {
    port: SerialPort,
    reader: Reader,
    writer: WritableStreamDefaultWriter,
    metadata: Metadata,
    accumulator: MeasurementAccumulator,
}

/// Reads from a port. A read that didn't complete (timed out, or the worker got a command) stays pending for the
/// next one, so no data is lost.
struct Reader {
    reader: ReadableStreamDefaultReader,
    pending: Option<JsFuture>,
}

impl Reader {
    async fn read(&mut self) -> Result<Vec<u8>> {
        let reader = &self.reader;
        let result = self.pending.get_or_insert_with(|| JsFuture::from(reader.read())).await;
        self.pending = None;
        let result = result.map_err(js_error)?;
        if Reflect::get(&result, &"done".into()).is_ok_and(|d| d.is_truthy()) {
            return Err("The serial port was closed".into());
        }
        let value = Reflect::get(&result, &"value".into()).map_err(js_error)?;
        Ok(value.dyn_into::<Uint8Array>().map_err(js_error)?.to_vec())
    }

    /// Read, or `None` after `ms` without data
    async fn read_timeout(&mut self, ms: u32) -> Result<Option<Vec<u8>>> {
        match select(pin!(self.read()), TimeoutFuture::new(ms)).await {
            Either::Left((bytes, _)) => bytes.map(Some),
            Either::Right(_) => Ok(None),
        }
    }
}

impl Port {
    /// Open the port and check that it's a PPK2 data port, by reading its metadata
    async fn open(port: SerialPort, mode: MeasurementMode, voltage: SourceVoltage, power: DevicePower) -> Result<Self> {
        let options = SerialOptions::new(115_200);
        options.set_buffer_size(BUFFER_SIZE);
        port.open(&options).await.map_err(js_error)?;
        // Required to work on Windows, as natively
        let signals = SerialOutputSignals::new();
        signals.set_data_terminal_ready(true);
        if let Err(e) = port.set_signals_with_signals(&signals).await {
            web_sys::console::warn_2(&"Failed to set DTR:".into(), &e);
        }
        let reader = port.readable().get_reader().unchecked_into::<ReadableStreamDefaultReader>();
        let writer = port.writable().get_writer().map_err(js_error)?;
        let mut port = Self {
            port,
            reader: Reader { reader, pending: None },
            writer,
            metadata: Metadata::default(),
            accumulator: MeasurementAccumulator::new(Metadata::default()),
        };
        match port.init(mode, voltage, power).await {
            Ok(()) => Ok(port),
            Err(e) => {
                port.close().await;
                Err(e)
            }
        }
    }

    async fn init(&mut self, mode: MeasurementMode, voltage: SourceVoltage, power: DevicePower) -> Result<()> {
        // The PPK2 may still be measuring, e.g. when another application didn't stop it: its samples would be
        // mistaken for the responses to the commands
        self.stop_streaming().await?;
        self.metadata = self.get_metadata().await?;
        self.send_command(Command::SetPowerMode(mode)).await?;
        self.set_voltage(voltage).await?;
        self.send_command(Command::DeviceRunningSet(power)).await?;
        self.start().await
    }

    async fn write(&self, bytes: &[u8]) -> Result<()> {
        JsFuture::from(self.writer.write_with_chunk(&Uint8Array::from(bytes)))
            .await
            .map(drop)
            .map_err(js_error)
    }

    /// Send a command and return the response
    async fn send_command(&mut self, command: Command) -> Result<Vec<u8>> {
        let bytes = Vec::from_iter(command.bytes());
        self.write(&bytes).await?;
        let mut response = Vec::with_capacity(command.expected_response_len());
        let start = Instant::now();
        while !command.response_complete(&response) {
            let left = RESPONSE_TIMEOUT.saturating_sub(start.elapsed());
            match self.reader.read_timeout(left.as_millis() as u32).await? {
                // E.g. samples of a running measurement instead of a response
                Some(_) if response.len() > 4096 => return Err(no_response(bytes[0])),
                Some(b) => response.extend_from_slice(&b),
                None => return Err(no_response(bytes[0])),
            }
        }
        Ok(response)
    }

    /// Stop a running measurement and discard its samples, until the PPK2 is quiet
    async fn stop_streaming(&mut self) -> Result<()> {
        let stop = Vec::from_iter(Command::AverageStop.bytes());
        let start = Instant::now();
        let mut last_stop: Option<Instant> = None;
        loop {
            if last_stop.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                self.write(&stop).await?;
                last_stop = Some(Instant::now());
            }
            match self.reader.read_timeout(50).await? {
                Some(_) if start.elapsed() > STOP_STREAMING_TIMEOUT => return Err(no_response(stop[0])),
                Some(_) => {}
                None => return Ok(()),
            }
        }
    }

    async fn get_metadata(&mut self) -> Result<Metadata> {
        let mut result = Err("No metadata".to_string());
        // Retry a few times, as the metadata command sometimes fails
        for _ in 0..3 {
            result = match self.send_command(Command::GetMetaData).await {
                Ok(response) => Metadata::from_bytes(&response).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if result.is_ok() {
                break;
            }
            // Discard the rest of a garbled response, or the samples of a measurement still running
            self.stop_streaming().await?;
        }
        result
    }

    async fn set_voltage(&mut self, voltage: SourceVoltage) -> Result<()> {
        self.send_command(Command::RegulatorSet(voltage)).await?;
        // The current calculation depends on the source voltage
        self.metadata.vdd = voltage.millivolts();
        Ok(())
    }

    /// Start measuring, with a new accumulator for the current metadata
    async fn start(&mut self) -> Result<()> {
        self.accumulator = MeasurementAccumulator::new(self.metadata.clone());
        self.send_command(Command::AverageStart).await.map(drop)
    }

    /// Stop measuring and close the port. Errors don't matter: it may be gone already.
    async fn close(self) {
        let _ = self.write(&Vec::from_iter(Command::AverageStop.bytes())).await;
        // Ends a pending read
        let _ = JsFuture::from(self.reader.reader.cancel()).await;
        self.reader.reader.release_lock();
        self.writer.release_lock();
        let _ = self.port.close().await;
    }
}

struct Worker {
    rx: UnboundedReceiver<Cmd>,
    shared: Arc<Mutex<Shared>>,
    ctx: eframe::egui::Context,
    port: Option<Port>,
    voltage: SourceVoltage,
    power: DevicePower,
    /// Whether the samples go into the recording
    recording: bool,
    samples: VecDeque<Measurement>,
}

enum Event {
    Cmd(Cmd),
    Data(Result<Vec<u8>>),
}

impl Worker {
    async fn run(mut self) {
        loop {
            let event = match &mut self.port {
                Some(port) => match select(pin!(port.reader.read()), self.rx.next()).await {
                    Either::Left((data, _)) => Event::Data(data),
                    Either::Right((cmd, _)) => Event::Cmd(cmd.unwrap_or(Cmd::Shutdown)),
                },
                None => Event::Cmd(self.rx.next().await.unwrap_or(Cmd::Shutdown)),
            };
            let cmd = match event {
                Event::Data(Ok(bytes)) => {
                    self.pump(&bytes);
                    continue;
                }
                Event::Data(Err(e)) => {
                    // E.g. the PPK2 was unplugged
                    self.disconnect().await;
                    self.fail(e);
                    continue;
                }
                Event::Cmd(cmd) => cmd,
            };

            if matches!(cmd, Cmd::Shutdown) {
                self.disconnect().await;
                return;
            }

            self.shared.lock().unwrap().error = None;
            if let Err(e) = self.handle(cmd).await {
                // Any error leaves the device disconnected
                self.disconnect().await;
                self.fail(e);
            }
            self.publish();
        }
    }

    fn fail(&mut self, error: String) {
        web_sys::console::error_1(&error.clone().into());
        self.shared.lock().unwrap().error = Some(error);
        self.publish();
    }

    /// Decode the received bytes into the live buffer and, while recording, the recording
    fn pump(&mut self, bytes: &[u8]) {
        let Some(port) = &mut self.port else { return };
        let missed = port.accumulator.feed_into(bytes, &mut self.samples);
        if missed > 0 {
            web_sys::console::warn_1(&format!("{missed} samples missed").into());
        }
        let mut shared = self.shared.lock().unwrap();
        for m in self.samples.drain(..) {
            let pins = (0..8).filter(|&i| m.pins.pin_is_high(i)).fold(0u8, |acc, i| acc | 1 << i);
            shared.live.push(m.micro_amps);
            if self.recording {
                shared.recording.push(m.micro_amps, pins);
            }
        }
    }

    async fn disconnect(&mut self) {
        if let Some(port) = self.port.take() {
            port.close().await;
        }
        self.recording = false;
    }

    async fn handle(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::Connect { port, mode } => {
                self.disconnect().await;
                let port = self.connect(port.as_deref() == Some(CHOOSE_PORT), mode).await?;
                let meta = &port.metadata;
                let mut shared = self.shared.lock().unwrap();
                shared.connection = Some(Connection {
                    port: "WebSerial".into(),
                    mode,
                    hw: meta.hw,
                    calibrated: meta.calibrated,
                    has_calibration_values: meta.has_calibration_values(),
                });
                shared.recording.source_mv = (mode == MeasurementMode::Source).then(|| self.voltage.millivolts());
                shared.live.clear();
                drop(shared);
                self.port = Some(port);
                self.recording = false;
            }
            Cmd::Disconnect => {
                self.disconnect().await;
                self.shared.lock().unwrap().connection = None;
            }
            Cmd::SetVoltage(mv) => {
                self.voltage = SourceVoltage::from_millivolts(mv);
                if let Some(port) = &mut self.port {
                    // Restart the measurement, as the current calculation depends on the source voltage
                    port.stop_streaming().await?;
                    port.set_voltage(self.voltage).await?;
                    port.start().await?;
                    let mut shared = self.shared.lock().unwrap();
                    if shared.recording.source_mv.is_some() {
                        shared.recording.source_mv = Some(self.voltage.millivolts());
                    }
                }
            }
            Cmd::SetPower(on) => {
                self.power = if on { DevicePower::Enabled } else { DevicePower::Disabled };
                // Without interrupting the measurement, to see e.g. the inrush current. No response to wait for.
                if let Some(port) = &self.port {
                    port.write(&Vec::from_iter(Command::DeviceRunningSet(self.power).bytes())).await?;
                }
            }
            Cmd::Start => {
                if self.port.is_some() {
                    self.shared.lock().unwrap().recording.resume();
                    self.recording = true;
                }
            }
            Cmd::Stop => self.recording = false,
            Cmd::Shutdown => unreachable!(),
        }
        Ok(())
    }

    /// Open the PPK2: one of the ports the user allowed this page before, else (or if `choose`) the port the user
    /// chooses now. The PPK2 has two serial ports: the one that answers is the data port.
    async fn connect(&mut self, choose: bool, mode: MeasurementMode) -> Result<Port> {
        let serial = web_sys::window()
            .map(|w| w.navigator().serial())
            .filter(|s| !s.is_undefined())
            .ok_or("This browser doesn't support WebSerial: use Chrome or Edge")?;
        let mut candidates = Vec::new();
        if !choose {
            let allowed = serial.get_ports().await.map_err(js_error)?;
            candidates.extend(allowed.iter().filter(|p| {
                let info = p.get_info();
                info.get_usb_vendor_id() == Some(PPK2_VID) && info.get_usb_product_id() == Some(PPK2_PID)
            }));
        }
        if candidates.is_empty() {
            let filter = SerialPortFilter::new();
            filter.set_usb_vendor_id(PPK2_VID);
            filter.set_usb_product_id(PPK2_PID);
            let options = SerialPortRequestOptions::new();
            options.set_filters(&[filter]);
            // Fails if the user cancels
            let port = serial.request_port_with_options(&options).await.map_err(js_error)?;
            candidates.push(port);
        }
        let mut error = String::new();
        for port in candidates {
            match Port::open(port, mode, self.voltage, self.power).await {
                Ok(port) => return Ok(port),
                Err(e) => error = e,
            }
        }
        Err(format!("{error} Choose the other PPK2 port (the data port has the lower number)?"))
    }

    /// Update the shared state from the worker state
    fn publish(&mut self) {
        let mut shared = self.shared.lock().unwrap();
        shared.streaming = self.port.is_some();
        shared.measuring = shared.streaming && self.recording;
        if !shared.streaming {
            self.recording = false;
            shared.connection = None;
        }
        drop(shared);
        self.ctx.request_repaint();
    }
}

fn no_response(command: u8) -> String {
    ppk2::Error::NoResponse(command).to_string()
}

fn js_error(e: JsValue) -> String {
    match e.dyn_ref::<js_sys::Error>() {
        Some(e) => String::from(e.message()),
        None => format!("{e:?}"),
    }
}
