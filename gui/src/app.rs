//! The user interface

use std::time::{Duration, Instant};

use eframe::egui::{self, containers::scroll_area::ScrollSource, PointerButton, RichText, Stroke};
use egui_plot::{
    log_grid_spacer, Axis, FilledArea, GridMark, Line, Plot, PlotBounds, PlotPoint, PlotPoints, PlotTransform, Points, Span,
    Text,
};
use ppk2::types::MeasurementMode;

use crate::{
    device::{Cmd, Device},
    theme::{self, Kind},
    meter::Meter,
    screenshot::{self, Screenshot},
    recording::{bytes_per_second, ConditionReport, Envelope, PinCondition, Recording, SAMPLE_RATE},
    stats::{format_axis_current, format_charge, format_charge_short, format_current, format_power, parse_current, Report},
};

/// Seconds of samples kept by default, and the choices
const DEFAULT_BUFFER_S: f64 = 300.;
const BUFFER_LENGTHS: [f64; 9] = [60., 300., 900., 1800., 3600., 7200., 14400., 28800., 86400.];
/// Choices of the number of samples combined into one stored value
const STORAGE_DECIMATIONS: [u64; 4] = [1, 10, 100, 1000];
/// Largest memory use (bytes) of the stored samples that can be chosen
const MAX_BUFFER_BYTES: f64 = 16e9;
/// Lowest current (µA) shown on a logarithmic axis
const LOG_FLOOR_UA: f64 = 0.01;
/// Selectable plot windows (s)
const WINDOWS: [f64; 9] = [0.01, 0.1, 0.5, 1., 2., 5., 10., 30., 60.];
/// Selectable readout averaging windows (s)
const AVERAGING: [f64; 6] = [0.1, 0.25, 0.5, 1., 2., 5.];
/// Shortest plot view (s)
const MIN_SPAN: f64 = 0.0005;
/// Minimum time between updates of the viewport statistics, as they can take a while for a long view
const VIEW_STATS_PERIOD: Duration = Duration::from_millis(250);
/// The auto scaled current axis only shrinks after the data fits a smaller range for this long
const AUTO_Y_SHRINK_DELAY: Duration = Duration::from_millis(1500);
/// Window size (points) that fits the compact view
const COMPACT_SIZE: egui::Vec2 = egui::vec2(980., 450.);
/// Zoom range and step of the compact view
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 2.;
const ZOOM_STEP: f32 = 0.1;
/// Distance (points) from an edge of the selection within which dragging moves that edge
const SELECTION_EDGE_PX: f32 = 6.;
/// Width of the vertical axes
const Y_AXIS_WIDTH: f32 = 72.;
/// While following, how far (s) the view stays behind the latest sample, as the samples arrive in bursts
const FOLLOW_LAG: f64 = 0.05;
/// Height of the digital channels plot
const DIGITAL_HEIGHT: f32 = 210.;
/// Storage key of the theme name
const THEME_KEY: &str = "theme";
/// Keyboard shortcuts and what they do, for the help window
const SHORTCUTS: [(&str, &str); 17] = [
    ("Ctrl + Enter", "Connect / disconnect"),
    ("P", "DUT power on / off"),
    ("Space", "Start / stop recording"),
    ("Ctrl + Del", "Clear the recording"),
    ("C", "Compact / full view"),
    ("F", "Follow the latest samples"),
    ("A", "Fit all: show the whole recording"),
    ("U", "Current axis automatic / manual"),
    ("L", "Logarithmic current axis"),
    ("K", "Keep 0 when zooming the current axis"),
    ("M", "Show the min/max band"),
    ("D", "Show the digital inputs"),
    ("Esc", "Close a window or menu, else clear the selection"),
    ("T", "Next color theme"),
    ("Ctrl + / Ctrl -", "Zoom in / out"),
    ("Ctrl + 0", "Reset the zoom"),
    ("F1", "Show these shortcuts"),
];

/// A serial port in the port list
struct Port {
    name: String,
    label: String,
}

/// Statistics computed from the samples of a time range, recomputed when the range or samples change
#[derive(Default)]
struct Cached {
    key: Option<[u64; 4]>,
    report: Option<Report>,
    at: Option<Instant>,
}

impl Cached {
    /// Recompute for `t0..t1` if anything changed, at most once per `period`. Returns whether it is up to date.
    fn update(&mut self, rec: &Recording, t0: f64, t1: f64, period: Duration) -> bool {
        let end = ((t1 * SAMPLE_RATE) as u64).min(rec.samples);
        let key = [t0.to_bits(), t1.to_bits(), rec.generation, end];
        if self.key == Some(key) {
            return true;
        }
        if self.at.is_some_and(|at| at.elapsed() < period) {
            return false;
        }
        self.report = rec.range_report(t0, t1);
        self.key = Some(key);
        self.at = Some(Instant::now());
        true
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// [ConditionReport] of a time range, recomputed when the range, samples or condition change
#[derive(Default)]
struct CachedCondition {
    key: Option<([u64; 4], PinCondition)>,
    report: Option<ConditionReport>,
    at: Option<Instant>,
}

impl CachedCondition {
    /// Recompute if anything changed, at most once per `period`. Returns whether it is up to date.
    fn update(&mut self, rec: &Recording, t0: f64, t1: f64, condition: PinCondition, period: Duration) -> bool {
        let end = ((t1 * SAMPLE_RATE) as u64).min(rec.samples);
        let key = ([t0.to_bits(), t1.to_bits(), rec.generation, end], condition);
        if self.key == Some(key) {
            return true;
        }
        if self.at.is_some_and(|at| at.elapsed() < period) {
            return false;
        }
        self.report = Some(rec.condition_report(t0, t1, condition));
        self.key = Some(key);
        self.at = Some(Instant::now());
        true
    }
}

/// What a drag in the plot does
enum Drag {
    /// Select a time range, from the time where the drag started
    Select(f64),
    /// Pan in time
    Pan,
    /// Pan in time and current
    Pan2d,
}

pub struct App {
    device: Device,
    ports: Vec<Port>,
    /// Selected port, `None` to find the PPK2 automatically
    port: Option<String>,
    mode: MeasurementMode,
    voltage_mv: u16,
    power: bool,
    interval_ms: u64,
    split: bool,
    split_ua: f32,

    /// Readout averaging window (s); the readout is updated once per window
    averaging_s: f64,
    /// Readout: average, min and max current (µA) and when it was last updated
    readout: Option<(f64, f64, f64)>,
    readout_at: Option<Instant>,
    meter: Meter,
    last_frame: Option<Instant>,

    /// Plot view (s) and whether it follows the latest samples, with a span of `window_s`
    t0: f64,
    t1: f64,
    follow: bool,
    window_s: f64,
    /// While following: the time (s) at the right of the view, and when
    follow_clock: Option<(f64, Instant)>,
    /// Width of the plot area (points), from the last frame
    plot_width: f32,
    drag: Option<Drag>,
    selection: Option<(f64, f64)>,
    selection_stats: Cached,
    view_stats: Cached,

    /// Current axis: automatic, or manual from `y_min` to `y_max` (µA)
    auto_y: bool,
    y_min: f64,
    y_max: f64,
    /// Auto scale range in plot coordinates, and since when the data fits a smaller range
    auto_range: Option<(f64, f64)>,
    shrink_since: Option<Instant>,
    log_y: bool,
    /// Zoom the current axis around 0 (logarithmic: around the bottom), so 0 stays in place
    keep_zero: bool,
    show_band: bool,
    show_digital: bool,
    screenshot: Option<Screenshot>,
    show_calibration: bool,
    show_shortcuts: bool,
    /// Time (s) of the plot point under the pointer, from the last frame
    hover_t: Option<f64>,
    /// Only the meter, the readout and the main controls; the window size and zoom before switching to it
    compact: bool,
    full_size: Option<egui::Vec2>,
    full_zoom: f32,
    /// Zoom of the compact mode, and the zoom the window was last sized for
    compact_zoom: f32,
    sized_for_zoom: Option<f32>,
    /// Whether compact mode started the measurement
    compact_started: bool,
    /// Statistics of the selection (or else the viewport) while the logic port pins meet this condition
    condition: PinCondition,
    condition_stats: CachedCondition,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let palette = cc.storage.and_then(|s| eframe::get_value::<String>(s, THEME_KEY));
        theme::init(&cc.egui_ctx, palette.as_deref());
        let mut app = Self {
            device: Device::spawn(cc.egui_ctx.clone(), Recording::new(DEFAULT_BUFFER_S, 1)),
            ports: Vec::new(),
            port: None,
            mode: MeasurementMode::Source,
            voltage_mv: 3000,
            power: false,
            interval_ms: 1000,
            split: false,
            split_ua: 1000.,
            averaging_s: 0.5,
            readout: None,
            readout_at: None,
            meter: Meter::default(),
            last_frame: None,
            t0: 0.,
            t1: 5.,
            follow: true,
            window_s: 5.,
            follow_clock: None,
            plot_width: 800.,
            drag: None,
            selection: None,
            selection_stats: Cached::default(),
            view_stats: Cached::default(),
            auto_y: true,
            y_min: 0.,
            y_max: 1000.,
            auto_range: None,
            shrink_since: None,
            log_y: false,
            keep_zero: true,
            show_band: true,
            show_digital: false,
            screenshot: Screenshot::from_env(),
            show_calibration: false,
            show_shortcuts: false,
            hover_t: None,
            compact: false,
            full_size: None,
            full_zoom: 1.,
            compact_zoom: 1.,
            sized_for_zoom: None,
            compact_started: false,
            condition: PinCondition::default(),
            condition_stats: CachedCondition::default(),
        };
        app.refresh_ports();
        if app.screenshot.is_some() {
            app.compact = std::env::var("PPK2_COMPACT").is_ok();
            if let Ok(name) = std::env::var("PPK2_THEME") {
                theme::init(&cc.egui_ctx, Some(&name));
            }
            let rec = &mut app.device.shared.lock().unwrap().recording;
            screenshot::demo_samples(rec);
            app.readout = rec.recent(app.averaging_s);
            app.meter.update(app.readout.map_or(0., |r| r.0), 1., 0.25);
            app.selection = Some((5.495, 5.56));
            app.show_digital = true;
            app.condition.set_level(0, Some(true));
        }
        app
    }

    fn refresh_ports(&mut self) {
        use serialport::SerialPortType::UsbPort;
        let data_port = ppk2::try_find_ppk2_port().ok();
        self.ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let is_ppk2 = matches!(&p.port_type, UsbPort(usb) if usb.vid == 0x1915 && usb.pid == 0xc00a);
                let label = match (is_ppk2, data_port.as_ref() == Some(&p.port_name)) {
                    (true, true) => format!("{} (PPK2)", p.port_name),
                    (true, false) => format!("{} (PPK2, not the data port)", p.port_name),
                    _ => p.port_name.clone(),
                };
                Port { name: p.port_name, label }
            })
            .collect();
    }

    fn clear(&mut self) {
        self.device.shared.lock().unwrap().recording.clear();
        self.selection = None;
        self.selection_stats.reset();
        self.view_stats.reset();
        self.follow = true;
        self.auto_range = None;
        self.readout = None;
        self.meter.reset();
    }

    fn toggle_connection(&mut self, connected: bool) {
        self.device.send(if connected {
            Cmd::Disconnect
        } else {
            Cmd::Connect { port: self.port.clone(), mode: self.mode }
        });
    }

    fn toggle_recording(&mut self, measuring: bool) {
        if measuring {
            self.device.send(Cmd::Stop);
        } else {
            self.device.send(Cmd::Start);
            self.follow = true;
        }
    }

    fn toggle_power(&mut self) {
        self.power = !self.power;
        self.device.send(Cmd::SetPower(self.power));
    }

    /// Switch between the compact and the full view, each with its own window size and zoom
    fn toggle_compact(&mut self, ctx: &egui::Context) {
        self.compact = !self.compact;
        if self.compact {
            self.full_size = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
            self.full_zoom = ctx.zoom_factor();
            ctx.set_zoom_factor(self.compact_zoom);
            // Sized once the zoom applies, see [App::size_compact_window]
            self.sized_for_zoom = None;
        } else {
            self.compact_zoom = ctx.zoom_factor();
            ctx.set_zoom_factor(self.full_zoom);
            let size = self.full_size.unwrap_or(egui::vec2(1280., 800.));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }

    /// The keyboard shortcuts, see [SHORTCUTS]. Single keys only while no text field has the focus.
    fn shortcuts(&mut self, ctx: &egui::Context, connected: bool, measuring: bool) {
        use egui::{Key, Modifiers};
        let (command, plain) = (Modifiers::COMMAND, Modifiers::NONE);
        if ctx.input_mut(|i| i.consume_key(command, Key::Enter)) {
            self.toggle_connection(connected);
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::Delete)) && !self.compact {
            self.clear();
        }
        if ctx.input_mut(|i| i.consume_key(plain, Key::F1)) {
            self.show_shortcuts = !self.show_shortcuts;
        }
        // Esc closes the windows first (menus and dropdowns close by themselves), before clearing the selection
        if (self.show_shortcuts || self.show_calibration) && ctx.input_mut(|i| i.consume_key(plain, Key::Escape)) {
            self.show_shortcuts = false;
            self.show_calibration = false;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let pressed = |key| ctx.input_mut(|i| i.consume_key(plain, key));
        if pressed(Key::P) && connected {
            self.toggle_power();
        }
        if pressed(Key::C) {
            self.toggle_compact(ctx);
        }
        if pressed(Key::T) {
            theme::select(ctx, theme::current() + 1);
        }
        if self.compact {
            return;
        }
        if pressed(Key::Space) && connected {
            self.toggle_recording(measuring);
        }
        if pressed(Key::F) {
            self.follow = !self.follow;
        }
        if pressed(Key::A) {
            self.fit_all();
        }
        if pressed(Key::U) {
            if self.auto_y {
                self.set_manual_y_from_auto();
            } else {
                self.auto_y = true;
            }
        }
        if pressed(Key::L) {
            self.log_y = !self.log_y;
            self.auto_range = None;
        }
        if pressed(Key::K) {
            self.keep_zero = !self.keep_zero;
        }
        if pressed(Key::M) {
            self.show_band = !self.show_band;
        }
        if pressed(Key::D) {
            self.show_digital = !self.show_digital;
        }
    }

    /// The color theme and the keyboard shortcuts
    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Settings", |ui| {
            ui.label(theme::title("Color theme"));
            for (i, palette) in theme::PALETTES.iter().enumerate() {
                if ui.selectable_label(theme::current() == i, palette.name).clicked() {
                    theme::select(ui.ctx(), i);
                }
            }
            ui.separator();
            if ui.add(egui::Button::new("Keyboard shortcuts").shortcut_text("F1")).clicked() {
                self.show_shortcuts = true;
                ui.close();
            }
        })
        .response
        .on_hover_text("Color theme (T) and keyboard shortcuts (F1)");
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.show_shortcuts)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts").num_columns(2).spacing([24., 6.]).striped(true).show(ui, |ui| {
                    for (key, action) in SHORTCUTS {
                        ui.label(theme::value(key, 15.));
                        ui.label(action);
                        ui.end_row();
                    }
                });
                ui.add_space(4.);
                ui.label(theme::title("Single keys work while no text field is being edited."));
            });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, connected: bool, measuring: bool, error: Option<String>) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().interact_size.y = 32.;
            ui.label(theme::value("PPK2", 22.).color(theme::p().accent));
            if !self.compact {
                ui.label(theme::title("Power Profiler"));
            }
            ui.add_space(20.);

            // Only needed to connect: hidden while connected, which leaves room for the status
            if !connected {
                let selected = match &self.port {
                    Some(name) => self.ports.iter().find(|p| &p.name == name).map_or(name.clone(), |p| p.label.clone()),
                    None => "Auto detect".into(),
                };
                // Narrower in compact mode, to fit its window
                let port_width = if self.compact { 140. } else { 200. };
                egui::ComboBox::from_id_salt("port").width(port_width).selected_text(selected).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.port, None, "Auto detect");
                    for p in &self.ports {
                        ui.selectable_value(&mut self.port, Some(p.name.clone()), &p.label);
                    }
                });
                if ui.button("⟳").on_hover_text("Refresh the serial ports").clicked() {
                    self.refresh_ports();
                }
                ui.add_space(12.);
                let source = self.mode == MeasurementMode::Source;
                if self.compact {
                    egui::ComboBox::from_id_salt("mode")
                        .width(120.)
                        .selected_text(if source { "Source meter" } else { "Ampere meter" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.mode, MeasurementMode::Source, "Source meter");
                            ui.selectable_value(&mut self.mode, MeasurementMode::Ampere, "Ampere meter");
                        });
                } else if theme::choice(ui, source, "Source meter")
                    .on_hover_text("The PPK2 powers the device under test")
                    .clicked()
                {
                    self.mode = MeasurementMode::Source;
                }
                if !self.compact && theme::choice(ui, !source, "Ampere meter")
                    .on_hover_text("Measure the current of an external supply, through VIN and VOUT")
                    .clicked()
                {
                    self.mode = MeasurementMode::Ampere;
                }
                ui.add_space(12.);
            }
            let (text, kind) = if connected { ("Disconnect", Kind::Normal) } else { ("Connect", Kind::Primary) };
            if ui
                .add(theme::big_button(text, "", kind, 110.).min_size(egui::vec2(110., 32.)))
                .on_hover_text("Ctrl + Enter")
                .clicked()
            {
                self.toggle_connection(connected);
            }

            // From the right: the view controls, then the status
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.view_controls(ui);
                ui.add_space(12.);
                let shared = self.device.shared.lock().unwrap();
                match &shared.connection {
                    Some(c) => {
                        if measuring {
                            ui.label(theme::value("● REC", 16.).color(theme::p().warn));
                            ui.add_space(8.);
                        }
                        let color = if c.has_calibration_values { theme::p().label } else { theme::p().warn };
                        if ui
                            .add(egui::Button::new(RichText::new("Calibration info").color(color)).frame(false))
                            .on_hover_text("About the calibration of the PPK2")
                            .clicked()
                        {
                            self.show_calibration = !self.show_calibration;
                        }
                        ui.label(theme::title(&format!("HW {}", c.hw)));
                        let mode = match c.mode {
                            MeasurementMode::Source => "source meter",
                            MeasurementMode::Ampere => "ampere meter",
                        };
                        ui.label(theme::title(mode));
                        ui.label(RichText::new(&c.port).color(theme::p().text));
                        ui.label(RichText::new("●").color(theme::p().accent));
                    }
                    // The Connect button says as much, and compact mode has no room
                    None if !self.compact => {
                        ui.label(theme::title("Not connected"));
                    }
                    None => {}
                }
                drop(shared);
                if let Some(e) = error {
                    ui.add_space(12.);
                    ui.add(egui::Label::new(RichText::new(e).color(theme::p().warn)).truncate());
                }
            });
        });
    }

    /// Compact or full view, the zoom of the compact view and the settings, laid out right to left
    fn view_controls(&mut self, ui: &mut egui::Ui) {
        self.settings_menu(ui);
        ui.add_space(4.);
        if self.compact {
            let ctx = ui.ctx().clone();
            let zoom = ctx.zoom_factor();
            if ui.button("Fit").on_hover_text("Size the window to fit the compact view").clicked() {
                self.sized_for_zoom = None;
            }
            if ui.button("+").on_hover_text("Larger (Ctrl + +)").clicked() {
                ctx.set_zoom_factor((zoom + ZOOM_STEP).min(MAX_ZOOM));
            }
            ui.label(theme::title(&format!("{:.0} %", zoom * 100.)));
            if ui.button("−").on_hover_text("Smaller (Ctrl + -)").clicked() {
                ctx.set_zoom_factor((zoom - ZOOM_STEP).max(MIN_ZOOM));
            }
        }
        let (text, hint) = if self.compact {
            ("Full view", "Plots, recording and all statistics")
        } else {
            ("Compact", "Only the meter, the readout and the supply")
        };
        if ui.button(theme::mnemonic(ui, text, 'c')).on_hover_text(format!("{hint} (C)")).clicked() {
            self.toggle_compact(ui.ctx());
        }
    }

    /// Per frame: apply the statistics settings, update the readout once per averaging window and move the needle
    /// The readout and the needle follow the live samples, also while not recording
    fn tick(&mut self, streaming: bool) {
        let now = Instant::now();
        let dt = self.last_frame.map_or(0., |t| (now - t).as_secs_f64());
        self.last_frame = Some(now);
        let shared = &mut *self.device.shared.lock().unwrap();
        // Settings take effect immediately, also for the running interval
        shared.recording.interval_ms = self.interval_ms;
        shared.recording.split_ua = self.split.then_some(self.split_ua);
        let due = self.readout_at.is_none_or(|at| at.elapsed().as_secs_f64() >= self.averaging_s);
        if streaming && due {
            self.readout = shared.live.recent(self.averaging_s);
            self.readout_at = Some(now);
        }
        if streaming {
            if let Some((avg, _, _)) = shared.live.recent(0.05) {
                self.meter.update(avg, dt, 0.25);
            }
        }
    }

    /// Source voltage and DUT power: the voltage above the DUT power button, or `inline` next to it
    fn supply_controls(&mut self, ui: &mut egui::Ui, connected: bool, inline: bool) {
        ui.add_enabled_ui(connected, |ui| {
            if inline {
                ui.horizontal(|ui| {
                    // Room for the voltage on the right
                    let width = (ui.available_width() - 190.).max(120.);
                    self.power_button(ui, width);
                    self.voltage_control(ui);
                });
            } else {
                self.voltage_control(ui);
                self.power_button(ui, ui.available_width());
            }
        });
    }

    fn voltage_control(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(self.mode == MeasurementMode::Source, |ui| {
            ui.horizontal(|ui| {
                ui.label("Voltage");
                let r = ui.add(
                    egui::DragValue::new(&mut self.voltage_mv)
                        .range(800..=5000)
                        .speed(10)
                        .suffix(" mV"),
                );
                // Also disabled while not connected, so only explain it as ampere meter
                let r = if self.mode == MeasurementMode::Ampere {
                    r.on_disabled_hover_text("Ampere meter: the device under test has an external supply")
                } else {
                    r
                };
                // Apply once done dragging or typing, as it restarts the measurement
                if r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus()) {
                    self.device.send(Cmd::SetVoltage(self.voltage_mv));
                }
                // Compact mode has no room for the note, the hover text has it too
                if self.mode == MeasurementMode::Ampere && !self.compact {
                    ui.label(theme::title("external supply"));
                }
            });
        });
    }

    fn power_button(&mut self, ui: &mut egui::Ui, width: f32) {
        let (text, kind) = if self.power {
            ("DUT power on", Kind::Primary)
        } else {
            ("DUT power off", Kind::Normal)
        };
        if ui.add(theme::big_button(text, "P", kind, width)).clicked() {
            self.toggle_power();
        }
    }

    /// How long and at what rate the samples are kept, with the memory that takes
    fn buffer_settings(&mut self, ui: &mut egui::Ui) {
        let (mut max_seconds, mut samples_per_stored) = {
            let rec = &self.device.shared.lock().unwrap().recording;
            (rec.max_seconds(), rec.samples_per_stored())
        };
        let fits = |seconds: f64, n: u64| bytes_per_second(n) * seconds <= MAX_BUFFER_BYTES;
        let (old_seconds, old_n) = (max_seconds, samples_per_stored);
        ui.horizontal(|ui| {
            ui.label(theme::title("Keep"));
            egui::ComboBox::from_id_salt("buffer length")
                .width(70.)
                .selected_text(format_buffer(max_seconds))
                .show_ui(ui, |ui| {
                    for seconds in BUFFER_LENGTHS {
                        ui.add_enabled_ui(fits(seconds, samples_per_stored), |ui| {
                            ui.selectable_value(&mut max_seconds, seconds, format_buffer(seconds));
                        });
                    }
                })
                .response
                .on_hover_text("How long the samples are kept for the plot and the selection and viewport statistics");
            ui.label(theme::title("at"));
            egui::ComboBox::from_id_salt("storage rate")
                .width(90.)
                .selected_text(format_rate(samples_per_stored))
                .show_ui(ui, |ui| {
                    for n in STORAGE_DECIMATIONS {
                        ui.add_enabled_ui(fits(max_seconds, n), |ui| {
                            ui.selectable_value(&mut samples_per_stored, n, format_rate(n));
                        });
                    }
                })
                .response
                .on_hover_text(
                    "Stored values per second. The PPK2 always samples at 100 kS/s: a lower rate stores the average, \
                     minimum and maximum of each group of samples. Changing it clears the recording.",
                );
            ui.label(theme::title(&format!("≈ {}", format_bytes(bytes_per_second(samples_per_stored) * max_seconds))));
        });
        if max_seconds != old_seconds {
            self.device.shared.lock().unwrap().recording.set_max_seconds(max_seconds);
        }
        if samples_per_stored != old_n {
            self.device.shared.lock().unwrap().recording.set_samples_per_stored(samples_per_stored);
            self.clear();
        }
    }

    /// Start/Stop and Clear
    fn recording_buttons(&mut self, ui: &mut egui::Ui, connected: bool, measuring: bool) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.;
            ui.add_enabled_ui(connected, |ui| {
                let button = if measuring {
                    ui.add(theme::big_button("Stop", "Space", Kind::Danger, width))
                } else {
                    ui.add(theme::big_button("Start", "Space", Kind::Primary, width))
                        .on_hover_text("Start or continue the recording")
                };
                if button.clicked() {
                    self.toggle_recording(measuring);
                }
            });
            if ui
                .add(theme::big_button("Clear", "Ctrl+Del", Kind::Normal, width))
                .on_hover_text("Clear the recording")
                .clicked()
            {
                self.clear();
            }
        });
    }

    /// The averaged current (and power) with the averaging window, and its minimum and maximum
    fn readout(&mut self, ui: &mut egui::Ui, source_mv: Option<u16>, size: f32) {
        ui.horizontal(|ui| {
            ui.label(theme::title("Current"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("averaging")
                    .width(70.)
                    .selected_text(format_seconds(self.averaging_s))
                    .show_ui(ui, |ui| {
                        for a in AVERAGING {
                            ui.selectable_value(&mut self.averaging_s, a, format_seconds(a));
                        }
                    })
                    .response
                    .on_hover_text("Averaging window; the readout is updated once per window");
                ui.label(theme::title("average over"));
            });
        });
        let avg = self.readout.map(|(avg, _, _)| avg);
        ui.label(theme::value(avg.map(format_current).unwrap_or_else(|| "—".into()), size));
        if let (Some(i), Some(mv)) = (avg, source_mv) {
            ui.label(theme::value(format_power(i * f64::from(mv) / 1000.), size / 2.).color(theme::p().label));
        }
        if let Some((_, min, max)) = self.readout {
            ui.label(theme::title(&format!("min {}   max {}", format_current(min), format_current(max))));
        }
    }

    /// Compact mode: the meter, and next to it the supply, the readout and the last interval. No recording
    /// controls: compact mode measures by itself, see [App::compact_measuring].
    fn compact_view(&mut self, ui: &mut egui::Ui, connected: bool) {
        let (source_mv, last) = {
            let rec = &self.device.shared.lock().unwrap().recording;
            (rec.source_mv, rec.reports.back().cloned())
        };
        ui.columns(2, |columns| {
            theme::card(&mut columns[0], |ui| {
                ui.vertical_centered(|ui| {
                    let range = self.readout.map(|(_, min, max)| (min, max));
                    self.meter.ui(ui, range, ui.available_width());
                });
            });
            column_scroll(&mut columns[1], "compact column", |ui| {
                theme::card(ui, |ui| self.supply_controls(ui, connected, true));
                theme::card(ui, |ui| self.readout(ui, source_mv, 48.));
                theme::card(ui, |ui| {
                    ui.label(theme::title("Last interval"));
                    match &last {
                        Some(r) => {
                            egui::Grid::new("compact interval").num_columns(4).spacing([10., 4.]).show(ui, |ui| {
                                report_cell(ui, "avg", format_current(r.avg_ua));
                                report_cell(ui, "max", format_current(r.max_ua.into()));
                                ui.end_row();
                                report_cell(ui, "rms", format_current(r.rms_ua));
                                report_cell(ui, "charge", format_charge_short(r.charge_uc));
                                ui.end_row();
                            });
                        }
                        None => {
                            ui.label(theme::title("—"));
                        }
                    }
                });
            });
        });
    }

    /// Compact mode has no recording controls, but measures while connected for the meter and statistics. The
    /// measurement it started stops again when leaving compact mode.
    fn compact_measuring(&mut self, connected: bool, measuring: bool) {
        if self.compact && connected && !measuring && !self.compact_started {
            self.device.send(Cmd::Start);
            self.compact_started = true;
        } else if !self.compact && self.compact_started {
            if measuring {
                self.device.send(Cmd::Stop);
            }
            self.compact_started = false;
        } else if !connected {
            self.compact_started = false;
        }
    }

    /// In compact mode, size the window to fit the compact view whenever the zoom changes (also by Ctrl + +/-)
    fn size_compact_window(&mut self, ctx: &egui::Context) {
        if !self.compact {
            return;
        }
        let zoom = ctx.zoom_factor();
        if self.sized_for_zoom != Some(zoom) {
            // In points, which scale with the zoom
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(COMPACT_SIZE));
            self.sized_for_zoom = Some(zoom);
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, connected: bool, measuring: bool) {
        let (recorded, source_mv, last, totals, history) = {
            let rec = &self.device.shared.lock().unwrap().recording;
            let history: Vec<_> = rec.reports.iter().rev().map(|r| (r.avg_ua, r.max_ua, r.charge_uc)).collect();
            (rec.duration_s(), rec.source_mv, rec.reports.back().cloned(), rec.totals(), history)
        };

        // Drag to scroll also without a detected touch screen: touches may arrive as mouse input
        let scroll_area = egui::ScrollArea::vertical().auto_shrink(false).scroll_source(ScrollSource::ALL);
        scroll_area.show(ui, |ui| {
            theme::card(ui, |ui| {
                ui.label(theme::title("Supply"));
                self.supply_controls(ui, connected, false);
            });

            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::title("Recording"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::value(format_recorded(recorded), 16.));
                    });
                });
                self.recording_buttons(ui, connected, measuring);
                self.buffer_settings(ui);
            });

            theme::card(ui, |ui| {
                self.readout(ui, source_mv, 40.);
                ui.add_space(4.);
                ui.vertical_centered(|ui| self.meter.ui(ui, self.readout.map(|(_, min, max)| (min, max)), 320.));
            });

            theme::card(ui, |ui| {
                ui.label(theme::title("Statistics"));
                egui::Grid::new("stats settings").num_columns(2).spacing([12., 8.]).show(ui, |ui| {
                    ui.label("Interval");
                    ui.add(egui::DragValue::new(&mut self.interval_ms).range(10..=600_000).speed(10).suffix(" ms"));
                    ui.end_row();
                    ui.checkbox(&mut self.split, "Split at")
                        .on_hover_text("Split into active (at or above this current) and sleep parts");
                    ui.add_enabled(
                        self.split,
                        egui::DragValue::new(&mut self.split_ua).range(0.0..=1e6).speed(1.).suffix(" µA"),
                    );
                    ui.end_row();
                });
                ui.add_space(4.);

                section(ui, "Last interval", true, |ui| match &last {
                    Some(r) => report_grid(ui, "last", r, true),
                    None => {
                        ui.label(theme::title("—"));
                    }
                });
                section(ui, "Whole recording", false, |ui| match &totals {
                    Some(r) => report_grid(ui, "totals", r, false),
                    None => {
                        ui.label(theme::title("—"));
                    }
                });
                section(ui, "Interval history", false, |ui| {
                    egui::Grid::new("history").striped(true).spacing([12., 4.]).show(ui, |ui| {
                        for h in ["#", "avg", "max", "charge"] {
                            ui.label(theme::title(h));
                        }
                        ui.end_row();
                        for (i, (avg, max, charge)) in history.iter().enumerate() {
                            ui.label(theme::title(&format!("-{i}")));
                            ui.label(format_current(*avg));
                            ui.label(format_current(f64::from(*max)));
                            ui.label(format!("{charge:.3} µC"));
                            ui.end_row();
                        }
                    });
                });
            });
        });
    }

    fn plot_toolbar(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let text = theme::mnemonic(ui, "Follow", 'f');
                theme::toggle(ui, &mut self.follow, text).on_hover_text("Follow the latest samples (F)");
                egui::ComboBox::from_id_salt("window")
                    .width(70.)
                    .selected_text(format_seconds(self.window_s))
                    .show_ui(ui, |ui| {
                        for w in WINDOWS {
                            if ui.selectable_value(&mut self.window_s, w, format_seconds(w)).clicked() {
                                self.follow = true;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Time window while following");
                if ui
                    .button(theme::mnemonic(ui, "Fit all", 'a'))
                    .on_hover_text("Show the whole recording (A, or double click in the plot)")
                    .clicked()
                {
                    self.fit_all();
                }

                ui.add_space(16.);
                ui.label(theme::title("Current axis"));
                let text = theme::mnemonic(ui, "Auto", 'u');
                if theme::choice(ui, self.auto_y, text).on_hover_text("Automatic current axis (U)").clicked() {
                    self.auto_y = true;
                }
                if theme::choice(ui, !self.auto_y, "Manual").on_hover_text("Manual current axis (U)").clicked()
                    && self.auto_y
                {
                    self.set_manual_y_from_auto();
                }
                if !self.auto_y {
                    let speed = ((self.y_max - self.y_min).abs() * 0.005).max(0.001);
                    for (i, v) in [&mut self.y_min, &mut self.y_max].into_iter().enumerate() {
                        if i == 1 {
                            ui.label(theme::title("to"));
                        }
                        ui.add(
                            egui::DragValue::new(v)
                                .speed(speed)
                                .custom_formatter(|v, _| format_current(v))
                                .custom_parser(parse_current),
                        )
                        .on_hover_text("e.g. 0, 500 nA or 1.5 mA");
                    }
                }
                let text = theme::mnemonic(ui, "Log", 'l');
                if theme::toggle(ui, &mut self.log_y, text).on_hover_text("Logarithmic current axis (L)").changed() {
                    self.auto_range = None;
                }
                let text = theme::mnemonic(ui, "Keep 0", 'k');
                theme::toggle(ui, &mut self.keep_zero, text).on_hover_text(
                    "Zoom the current axis around 0, so 0 stays in place (log scale: around the bottom). \
                     Off: zoom around the pointer (K)",
                );
                let text = theme::mnemonic(ui, "Min/max", 'm');
                theme::toggle(ui, &mut self.show_band, text)
                    .on_hover_text("Show the minimum and maximum, not only the average (M)");

                ui.add_space(16.);
                let text = theme::mnemonic(ui, "Digital", 'd');
                theme::toggle(ui, &mut self.show_digital, text).on_hover_text("Show the logic port inputs D0 to D7 (D)");
            });
        });
    }

    fn fit_all(&mut self) {
        let rec = &self.device.shared.lock().unwrap().recording;
        self.t0 = rec.start_s();
        self.t1 = rec.duration_s().max(self.t0 + MIN_SPAN);
        self.follow = false;
    }

    fn set_manual_y_from_auto(&mut self) {
        if let Some((lo, hi)) = self.auto_range {
            self.y_min = self.current_of(lo);
            self.y_max = self.current_of(hi);
        }
        self.auto_y = false;
    }

    fn y_of(&self, micro_amps: f64) -> f64 {
        to_y(self.log_y, micro_amps)
    }

    fn current_of(&self, y: f64) -> f64 {
        from_y(self.log_y, y)
    }

    /// Range of the auto scaled current axis (plot coordinates) for data from `lo` to `hi` (plot coordinates).
    /// The range is rounded to nice values, grows immediately and only shrinks after a while, so it is steady.
    fn auto_y_range(&mut self, data: Option<(f64, f64)>) -> (f64, f64) {
        let target = match data {
            Some((lo, hi)) if self.log_y => (lo.floor(), hi.ceil().max(lo.floor() + 1.)),
            Some((lo, hi)) => {
                let top = if hi > 0. { nice_ceil(hi * 1.05) } else { 1. };
                let bottom = if lo < 0. { -nice_ceil(-lo * 1.05) } else { 0. };
                (bottom, top)
            }
            None => self.auto_range.unwrap_or(if self.log_y { (-1., 3.) } else { (0., 1000.) }),
        };
        let range = match self.auto_range {
            Some(cur) if target.0 < cur.0 || target.1 > cur.1 => {
                self.shrink_since = None;
                (target.0.min(cur.0), target.1.max(cur.1))
            }
            Some(cur) if target != cur => {
                let since = *self.shrink_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= AUTO_Y_SHRINK_DELAY {
                    self.shrink_since = None;
                    target
                } else {
                    cur
                }
            }
            _ => {
                self.shrink_since = None;
                target
            }
        };
        self.auto_range = Some(range);
        range
    }

    fn plots(&mut self, ui: &mut egui::Ui, measuring: bool) {
        self.plot_toolbar(ui);

        let shared_arc = self.device.shared.clone();
        let shared = shared_arc.lock().unwrap();
        let rec = &shared.recording;
        let end = rec.duration_s();
        if self.follow {
            // The samples arrive in bursts: scroll smoothly in real time, a bit behind the latest sample
            let now = Instant::now();
            let target = end - FOLLOW_LAG;
            let t = match self.follow_clock {
                Some((t, at)) if measuring => {
                    let predicted = t + (now - at).as_secs_f64();
                    let error = target - predicted;
                    if error.abs() > 0.25 { target } else { (predicted + error * 0.1).min(end) }
                }
                _ => end,
            };
            self.follow_clock = Some((t, now));
            self.t1 = t.max(self.window_s);
            self.t0 = self.t1 - self.window_s;
        } else {
            self.follow_clock = None;
        }

        // One point per physical pixel, with the view at a whole pixel: moving the view then moves the plot by
        // whole pixels, while every point keeps its content (no aliasing jitter)
        let ppp = f64::from(ui.ctx().pixels_per_point());
        let columns = (f64::from(self.plot_width) * ppp).round().max(100.);
        let span = self.t1 - self.t0;
        let pixel = span / columns;
        let t0 = (self.t0 / pixel).round() * pixel;
        let t1 = t0 + span;
        let env = rec.envelope(t0, t1, columns as usize);

        // Current axis range
        let log_y = self.log_y;
        let data = (!env.t.is_empty()).then(|| {
            let (lo, hi) = if self.show_band { (&env.min, &env.max) } else { (&env.avg, &env.avg) };
            let lo = lo.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = hi.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            (to_y(log_y, lo), to_y(log_y, hi))
        });
        let auto = self.auto_y_range(data);
        let (y0, y1) = if self.auto_y {
            auto
        } else {
            let (lo, hi) = (self.y_of(self.y_min), self.y_of(self.y_max));
            if hi > lo { (lo, hi) } else { (lo, lo + 1.) }
        };

        let digital_height = if self.show_digital { DIGITAL_HEIGHT + ui.spacing().item_spacing.y } else { 0. };
        let stats_height = range_stats_height(ui, self.split, self.show_digital);
        let height = (ui.available_height() - digital_height - stats_height).max(100.);
        let selection = self.selection;
        let show_band = self.show_band;

        let current = interaction_plot("current", height, true)
            .x_axis_label(if self.show_digital { "" } else { "time (s)" })
            .show_axes([!self.show_digital, true])
            .y_axis_min_width(Y_AXIS_WIDTH)
            .y_axis_formatter(move |mark, _| {
                if log_y {
                    format_axis_current(from_y(log_y, mark.value), from_y(log_y, mark.value) / 10.)
                } else {
                    format_axis_current(mark.value, mark.step_size)
                }
            })
            .show_x(false)
            .show_y(false)
            .show_crosshair(false);
        let current = if log_y { current.y_grid_spacer(log_grid_spacer(10)) } else { current };
        let hover_t = self.hover_t.take();
        let current = current.show(ui, |pui| {
            pui.set_plot_bounds(PlotBounds::from_min_max([t0, y0], [t1, y1]));
            if let Some((s0, s1)) = selection {
                pui.span(selection_span(s0, s1));
            }
            if let Some(t) = hover_t {
                pui.vline(cursor_line(t));
            }
            if env.t.is_empty() {
                return None;
            }
            let hovered = pui.pointer_coordinate().and_then(|p| nearest_point(&env, p.x));
            if show_band {
                let min: Vec<f64> = env.min.iter().map(|&v| to_y(log_y, v)).collect();
                let max: Vec<f64> = env.max.iter().map(|&v| to_y(log_y, v)).collect();
                pui.add(
                    FilledArea::new("min/max", &env.t, &min, &max)
                        .fill_color(theme::p().band.gamma_multiply(0.3))
                        .allow_hover(false),
                );
            }
            let points: Vec<[f64; 2]> = env.t.iter().zip(&env.avg).map(|(&t, &v)| [t, to_y(log_y, v)]).collect();
            pui.line(Line::new("average", PlotPoints::new(points)).color(theme::p().accent).width(1.5));
            if let Some(i) = hovered {
                pui.points(
                    Points::new("", vec![[env.t[i], to_y(log_y, env.avg[i])]])
                        .radius(4.5)
                        .color(theme::p().text),
                );
            }
            hovered
        });

        let digital = self.show_digital.then(|| {
            interaction_plot("digital", DIGITAL_HEIGHT, false)
                .x_axis_label("time (s)")
                .y_axis_min_width(Y_AXIS_WIDTH)
                .y_grid_spacer(|_| (0..8).map(|i| GridMark { value: i as f64 + 0.5, step_size: 1. }).collect())
                .y_axis_formatter(|_, _| String::new())
                .show_x(false)
                .show_y(false)
                .show_crosshair(false)
                .show(ui, |pui| {
                    pui.set_plot_bounds(PlotBounds::from_min_max([t0, 0.], [t1, 8.]));
                    if let Some((s0, s1)) = selection {
                        pui.span(selection_span(s0, s1));
                    }
                    if let Some(t) = hover_t {
                        pui.vline(cursor_line(t));
                    }
                    for (pin, color) in theme::p().digital.iter().enumerate() {
                        pui.line(Line::new(format!("D{pin}"), digital_points(&env, pin)).color(*color).width(1.5));
                        let label = PlotPoint::new(t0 + (t1 - t0) * 0.004, (7 - pin) as f64 + 0.2);
                        pui.text(
                            Text::new(format!("D{pin} label"), label, RichText::new(format!("D{pin}")).color(*color))
                                .anchor(egui::Align2::LEFT_BOTTOM),
                        );
                    }
                    pui.pointer_coordinate().and_then(|p| nearest_point(&env, p.x))
                })
        });

        // Labels with the measured values at the pointer's time, following the pointer (a tooltip only shows
        // once the pointer stands still)
        if let Some(i) = current.inner {
            self.hover_t = Some(env.t[i]);
            let mut lines = vec![
                (format_point_time(&env, i), theme::p().label, 13.),
                (format_current(env.avg[i]), theme::p().text, 17.),
            ];
            if show_band && env.min[i] != env.max[i] {
                lines.push((format!("min {}", format_current(env.min[i])), theme::p().label, 13.));
                lines.push((format!("max {}", format_current(env.max[i])), theme::p().label, 13.));
            }
            let marker = current.transform.position_from_point(&PlotPoint::new(env.t[i], to_y(log_y, env.avg[i])));
            value_label(ui, *current.transform.frame(), marker, &lines);
        }
        if let Some((d, i)) = digital.as_ref().and_then(|d| d.inner.map(|i| (d, i))) {
            self.hover_t = Some(env.t[i]);
            let mut lines = vec![(format_point_time(&env, i), theme::p().label, 13.)];
            for (pin, color) in theme::p().digital.iter().enumerate() {
                let level = match (env.high[i] >> pin & 1 != 0, env.low[i] >> pin & 1 != 0) {
                    (true, true) => "toggling",
                    (true, false) => "1",
                    _ => "0",
                };
                lines.push((format!("D{pin}  {level}"), *color, 13.));
            }
            if let Some(pointer) = d.response.hover_pos() {
                value_label(ui, *d.transform.frame(), pointer, &lines);
            }
        }

        // Statistics of the selection and the viewport
        if let Some((s0, s1)) = self.selection {
            // While dragging, a long selection is updated now and then only
            let period = if self.drag.is_some() { VIEW_STATS_PERIOD } else { Duration::ZERO };
            if !self.selection_stats.update(rec, s0, s1, period) {
                ui.ctx().request_repaint_after(VIEW_STATS_PERIOD);
            }
        }
        if !self.view_stats.update(rec, t0.max(0.), t1, VIEW_STATS_PERIOD) {
            ui.ctx().request_repaint_after(VIEW_STATS_PERIOD);
        }
        if self.show_digital && self.condition.is_set() {
            let (c0, c1) = self.selection.unwrap_or((t0.max(0.), t1));
            if !self.condition_stats.update(rec, c0, c1, self.condition, VIEW_STATS_PERIOD) {
                ui.ctx().request_repaint_after(VIEW_STATS_PERIOD);
            }
        }
        let start = rec.start_s();
        drop(shared);
        self.range_stats(ui);

        self.plot_width = current.transform.frame().width();
        self.axis_drags(&current.transform, [t0, y0], [t1, y1], true);
        self.interact(ui, &current.response, &current.transform, true, start, end, measuring);
        if let Some(d) = digital {
            self.axis_drags(&d.transform, [t0, 0.], [t1, 8.], false);
            self.interact(ui, &d.response, &d.transform, false, start, end, measuring);
        }
    }

    /// Take over the view after dragging an axis (labels) to scale it, which the plot handles itself
    fn axis_drags(&mut self, transform: &PlotTransform, min: [f64; 2], max: [f64; 2], current_axis: bool) {
        let b = transform.bounds();
        let changed = |d: usize| {
            let eps = (max[d] - min[d]).abs() * 1e-9;
            (b.min()[d] - min[d]).abs() > eps || (b.max()[d] - max[d]).abs() > eps
        };
        if changed(0) {
            let span = (b.max()[0] - b.min()[0]).clamp(MIN_SPAN, self.buffer_seconds());
            if self.follow {
                self.window_s = span;
            } else {
                self.t0 = b.min()[0];
                self.t1 = self.t0 + span;
            }
        }
        if current_axis && changed(1) {
            let factor = (b.max()[1] - b.min()[1]) / (max[1] - min[1]);
            self.auto_y = false;
            self.y_min = self.current_of(min[1]);
            self.y_max = self.current_of(max[1]);
            self.zoom_y(factor, (b.min()[1] + b.max()[1]) / 2.);
        }
    }

    /// Scale the (manual) current axis by `factor`, around `around` (plot coordinates), or around 0 with
    /// [App::keep_zero]
    fn zoom_y(&mut self, factor: f64, around: f64) {
        let (lo, hi) = (self.y_of(self.y_min), self.y_of(self.y_max));
        let anchor = match (self.keep_zero, self.log_y) {
            (false, _) => around,
            (true, true) => lo,
            // 0 or, when out of view, the edge closest to it
            (true, false) => 0f64.clamp(lo, hi),
        };
        self.y_min = self.current_of(anchor - (anchor - lo) * factor);
        self.y_max = self.current_of(anchor + (hi - anchor) * factor);
    }

    /// Zoom, pan and select by mouse:
    /// - scroll to zoom in time, ctrl + scroll to zoom the current axis, shift + scroll to pan
    /// - drag (any button) to pan, ctrl + drag to pan in current as well, shift + drag to select, drag an edge of the
    ///   selection to move it
    /// - click or Esc to clear the selection, double click to show the whole recording, auto scaled
    #[allow(clippy::too_many_arguments)]
    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        transform: &PlotTransform,
        current_axis: bool,
        start: f64,
        end: f64,
        measuring: bool,
    ) {
        let pointer = response.hover_pos().map(|p| transform.value_from_position(p));
        let span = self.t1 - self.t0;
        let seconds_per_px = transform.dvalue_dpos()[0];

        if response.hovered() {
            // egui turns ctrl + scroll into a zoom and shift + scroll into a horizontal scroll
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if scroll.y != 0. {
                let new_span = (span * (-f64::from(scroll.y) * 0.003).exp()).clamp(MIN_SPAN, self.buffer_seconds());
                if self.follow {
                    self.window_s = new_span;
                } else if let Some(p) = pointer {
                    let f = new_span / span;
                    self.t0 = p.x - (p.x - self.t0) * f;
                    self.t1 = p.x + (self.t1 - p.x) * f;
                }
            }
            if scroll.x != 0. {
                self.pan(-f64::from(scroll.x) * seconds_per_px);
            }
            if zoom != 1. && current_axis {
                if let Some(p) = pointer {
                    if self.auto_y {
                        self.set_manual_y_from_auto();
                    }
                    self.zoom_y(1. / f64::from(zoom), p.y);
                }
            }
        }

        // The other edge of the selection, if `x` (screen) is on one of its edges
        let selection_edge = |x: f32| {
            let (s0, s1) = self.selection?;
            let near = |s: f64| (transform.position_from_point_x(s) - x).abs() <= SELECTION_EDGE_PX;
            if near(s1) {
                Some(s0)
            } else if near(s0) {
                Some(s1)
            } else {
                None
            }
        };
        if self.drag.is_none() && response.hover_pos().is_some_and(|p| selection_edge(p.x).is_some()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if response.drag_started() {
            let shift = ui.input(|i| i.modifiers.shift);
            let origin = ui.input(|i| i.pointer.press_origin());
            let edge = origin.and_then(|p| selection_edge(p.x));
            self.drag = if let (Some(other), true) = (edge, response.drag_started_by(PointerButton::Primary)) {
                Some(Drag::Select(other))
            } else if shift && response.drag_started_by(PointerButton::Primary) {
                origin.map(|p| transform.value_from_position(p).x).or(pointer.map(|p| p.x)).map(Drag::Select)
            } else if current_axis && ui.input(|i| i.modifiers.command) {
                // Free panning: the current axis can't be automatic or kept at 0 then
                if self.auto_y {
                    self.set_manual_y_from_auto();
                }
                self.keep_zero = false;
                Some(Drag::Pan2d)
            } else {
                Some(Drag::Pan)
            };
        }
        if matches!(self.drag, Some(Drag::Select(_))) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if response.dragged() {
            match self.drag {
                Some(Drag::Select(anchor)) => {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()).map(|p| transform.value_from_position(p)) {
                        let x = p.x.clamp(start, end);
                        let anchor = anchor.clamp(start, end);
                        self.selection = (x != anchor).then(|| (anchor.min(x), anchor.max(x)));
                    }
                }
                Some(Drag::Pan) => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    self.pan(-f64::from(response.drag_delta().x) * seconds_per_px);
                }
                Some(Drag::Pan2d) => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let delta = response.drag_delta();
                    self.pan(-f64::from(delta.x) * seconds_per_px);
                    let dy = -f64::from(delta.y) * transform.dvalue_dpos()[1];
                    let (lo, hi) = (self.y_of(self.y_min), self.y_of(self.y_max));
                    self.y_min = self.current_of(lo + dy);
                    self.y_max = self.current_of(hi + dy);
                }
                None => {}
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if response.double_clicked() {
            self.fit_all();
            self.auto_y = true;
        } else if response.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selection = None;
        }

        // Keep the view sensible: not before the start, not far beyond the end
        if !self.follow {
            let span = (self.t1 - self.t0).clamp(MIN_SPAN, self.buffer_seconds());
            let latest = if measuring { end + span } else { end.max(span) };
            self.t0 = self.t0.clamp(start.min(latest - span), latest - span);
            self.t1 = self.t0 + span;
        }
    }

    fn calibration_window(&mut self, ctx: &egui::Context) {
        let connection = self.device.shared.lock().unwrap().connection.clone();
        let Some(c) = connection.filter(|_| self.show_calibration) else { return };
        egui::Window::new(theme::value("Calibration", 18.))
            .open(&mut self.show_calibration)
            .collapsible(false)
            .resizable(false)
            .default_width(460.)
            .anchor(egui::Align2::RIGHT_TOP, [-16., 60.])
            .show(ctx, |ui| {
                ui.label(
                    "The PPK2 is calibrated in the factory: the values for its five measurement ranges (resistors, \
                     gains and offsets) are stored on the device. This app reads them when connecting and applies \
                     them to every sample.",
                );
                ui.add_space(4.);
                if c.has_calibration_values {
                    ui.label(RichText::new("✔ This PPK2 has calibration values.").color(theme::p().accent));
                } else {
                    ui.label(
                        RichText::new(
                            "✖ This PPK2 has no calibration values: default values are used, so the measurements \
                             can be off. Contact Nordic Semiconductor.",
                        )
                        .color(theme::p().warn),
                    );
                }
                ui.add_space(4.);
                ui.label(theme::title(&format!(
                    "The PPK2 also reports a \"calibrated\" flag, {} on this one. Nordic's Power Profiler doesn't use \
                     this flag, and it is 0 on many calibrated devices.",
                    u8::from(c.calibrated)
                )));

                ui.add_space(8.);
                ui.label(theme::value("Checking the accuracy", 15.));
                ui.label(
                    "There is no procedure to calibrate a PPK2 yourself, but you can check it: as source meter, \
                     connect a precise (0.1 %) resistor between VOUT and GND, switch on the DUT power and compare \
                     the average current with V / R. For example at 3000 mV:",
                );
                egui::Grid::new("calibration loads").num_columns(2).spacing([24., 2.]).show(ui, |ui| {
                    for (r, i) in [("1 MΩ", "3 µA"), ("100 kΩ", "30 µA"), ("10 kΩ", "300 µA"), ("1 kΩ", "3 mA"), ("100 Ω", "30 mA")] {
                        ui.label(theme::title(r));
                        ui.label(i);
                        ui.end_row();
                    }
                });
                ui.label(theme::title(
                    "Check a few decades, so each of the measurement ranges of the PPK2 is covered.",
                ));
            });
    }

    /// Statistics of the selection and the viewport, side by side under the plots
    fn range_stats(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.);
        ui.columns(2, |columns| {
            theme::card(&mut columns[0], |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::title("Selection").color(theme::p().selection));
                    if let Some((s0, s1)) = self.selection {
                        ui.label(theme::value(format!("Δ {}", format_duration(s1 - s0)), 15.));
                        ui.label(theme::title(&format!("{s0:.6} to {s1:.6} s")));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✖").on_hover_text("Clear the selection (Esc)").clicked() {
                                self.selection = None;
                            }
                        });
                    }
                });
                match (self.selection, &self.selection_stats.report) {
                    (Some(_), Some(r)) => compact_report(ui, "selection", r),
                    _ => {
                        ui.label(theme::title("Shift + drag in the plot to select"));
                    }
                }
            });
            theme::card(&mut columns[1], |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::title("Viewport"));
                    ui.label(theme::value(format!("Δ {}", format_duration(self.t1 - self.t0.max(0.))), 15.));
                    ui.label(theme::title(&format!("{:.3} to {:.3} s", self.t0.max(0.), self.t1)));
                });
                match &self.view_stats.report {
                    Some(r) => compact_report(ui, "viewport", r),
                    None => {
                        ui.label(theme::title("—"));
                    }
                }
            });
        });
        if self.show_digital {
            self.condition_stats(ui);
        }
    }

    /// Statistics while the logic port pins meet a condition, of the selection or else the viewport
    fn condition_stats(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::title("When"));
                for (pin, color) in theme::p().digital.iter().enumerate() {
                    let level = self.condition.level(pin);
                    let text = match level {
                        None => format!("D{pin} any"),
                        Some(true) => format!("D{pin} = 1"),
                        Some(false) => format!("D{pin} = 0"),
                    };
                    let text = RichText::new(text).color(if level.is_some() { *color } else { theme::p().label });
                    if ui
                        .add(egui::Button::new(text).selected(level.is_some()))
                        .on_hover_text("Click: any, 1 (high), 0 (low)")
                        .clicked()
                    {
                        let next = match level {
                            None => Some(true),
                            Some(true) => Some(false),
                            Some(false) => None,
                        };
                        self.condition.set_level(pin, next);
                    }
                }
                ui.label(theme::title(if self.selection.is_some() { "in the selection" } else { "in the viewport" }));
            });
            if !self.condition.is_set() {
                ui.label(theme::title("Set one or more inputs to see the statistics while they are at that level"));
                return;
            }
            match self.condition_stats.report.as_ref() {
                Some(c) => {
                    egui::Grid::new("condition").num_columns(10).spacing([10., 4.]).show(ui, |ui| {
                        report_cell(ui, "time", format!("{:.2} %", c.fraction * 100.));
                        report_cell(ui, "times", format!("{}", c.occurrences));
                        match &c.report {
                            Some(r) => {
                                let each = r.duration_s / c.occurrences.max(1) as f64;
                                report_cell(ui, "each", format_duration(each));
                                report_cell(ui, "avg", format_current(r.avg_ua));
                                report_cell(ui, "max", format_current(r.max_ua.into()));
                                report_cell(ui, "charge", format_charge_short(r.charge_uc));
                            }
                            None => report_cell(ui, "", "never".into()),
                        }
                        ui.end_row();
                    });
                }
                None => {
                    ui.label(theme::title("—"));
                }
            }
        });
    }

    fn buffer_seconds(&self) -> f64 {
        self.device.shared.lock().unwrap().recording.max_seconds()
    }

    fn pan(&mut self, dt: f64) {
        self.follow = false;
        self.t0 += dt;
        self.t1 += dt;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (connected, measuring, streaming, error, mode) = {
            let shared = self.device.shared.lock().unwrap();
            let mode = shared.connection.as_ref().map(|c| c.mode);
            (shared.connection.is_some(), shared.measuring, shared.streaming, shared.error.clone(), mode)
        };
        if let Some(mode) = mode {
            self.mode = mode;
        }
        self.shortcuts(ui.ctx(), connected, measuring);

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(theme::p().bg).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| self.top_bar(ui, connected, measuring, error));
        self.tick(streaming);
        self.size_compact_window(ui.ctx());
        self.compact_measuring(connected, measuring);
        if self.compact {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::p().bg).inner_margin(egui::Margin::same(12)))
                .show(ui, |ui| self.compact_view(ui, connected));
        } else {
            egui::Panel::left("side")
                .frame(egui::Frame::new().fill(theme::p().bg).inner_margin(egui::Margin::same(12)))
                .resizable(true)
                .default_size(350.)
                .min_size(300.)
                .show(ui, |ui| self.side_panel(ui, connected, measuring));
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::p().bg).inner_margin(egui::Margin { left: 4, right: 16, top: 12, bottom: 12 }))
                .show(ui, |ui| self.plots(ui, measuring));
        }

        if streaming {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        if let Some(s) = &mut self.screenshot {
            s.frame(ui.ctx());
        }
        self.calibration_window(ui.ctx());
        self.shortcuts_window(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.screenshot.is_none() {
            eframe::set_value(storage, THEME_KEY, &theme::p().name);
        }
    }

    /// Only the theme is kept: the window size and zoom depend on the view
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn on_exit(&mut self) {
        // Stop a running measurement, rather than leave the PPK2 streaming
        self.device.shutdown();
    }
}

/// A plot without its own interaction, except dragging the axes to scale them: the view is set every frame,
/// see [App::interact] and [App::axis_drags]
fn interaction_plot(id: &str, height: f32, current_axis: bool) -> Plot<'static> {
    Plot::new(id)
        .height(height)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .allow_axis_zoom_drag([true, current_axis])
        .show_background(false)
        .grid_color(theme::p().divider)
}

fn selection_span(s0: f64, s1: f64) -> Span {
    // No name: it would be drawn in the plot
    Span::new("", s0..=s1)
        .axis(Axis::X)
        .fill(theme::p().selection.gamma_multiply(0.15))
        .border(Stroke::new(1., theme::p().selection.gamma_multiply(0.7)))
}

/// A label with `lines` of (text, color, size) next to `anchor` (screen), inside `frame`, on top of everything
fn value_label(ui: &egui::Ui, frame: egui::Rect, anchor: egui::Pos2, lines: &[(String, egui::Color32, f32)]) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("value label")));
    let galleys: Vec<_> = lines
        .iter()
        .map(|(text, color, size)| painter.layout_no_wrap(text.clone(), theme::bold(*size), *color))
        .collect();
    let width = galleys.iter().map(|g| g.size().x).fold(0., f32::max);
    let height: f32 = galleys.iter().map(|g| g.size().y).sum();
    let margin = egui::vec2(10., 6.);
    let size = egui::vec2(width, height) + 2. * margin;
    // Right of and above the anchor, or on the other side where it doesn't fit
    let mut min = anchor + egui::vec2(14., -14. - size.y);
    if min.x + size.x > frame.right() {
        min.x = anchor.x - 14. - size.x;
    }
    if min.y < frame.top() {
        min.y = anchor.y + 14.;
    }
    let rect = egui::Rect::from_min_size(min, size);
    painter.rect(rect, 6., theme::p().card.gamma_multiply(0.95), Stroke::new(1., theme::p().divider), egui::StrokeKind::Inside);
    let mut y = rect.top() + margin.y;
    for g in galleys {
        let h = g.size().y;
        painter.galley(egui::pos2(rect.left() + margin.x, y), g, theme::p().text);
        y += h;
    }
}

/// Vertical line at the hovered time, in all plots
fn cursor_line(t: f64) -> egui_plot::VLine {
    egui_plot::VLine::new("", t).color(theme::p().label.gamma_multiply(0.6)).width(1.)
}

/// Index of the plot point at time `t`, if within the plot data
fn nearest_point(env: &Envelope, t: f64) -> Option<usize> {
    let i = env.end.partition_point(|&end| end <= t);
    (i < env.t.len() && env.start[i] <= t).then_some(i)
}

/// Time of a plot point, with the precision of its duration
fn format_point_time(env: &Envelope, i: usize) -> String {
    let width = env.end[i] - env.start[i];
    let decimals = (-width.log10()).ceil().clamp(0., 6.) as usize;
    format!("{:.decimals$} s", env.t[i])
}

/// Line of a logic port pin: a step at its level, a block where it toggles within a point
fn digital_points(env: &Envelope, pin: usize) -> PlotPoints<'static> {
    let lane = (7 - pin) as f64;
    let (low, high) = (lane + 0.15, lane + 0.85);
    let mut points = Vec::with_capacity(env.t.len() * 2);
    for i in 0..env.t.len() {
        let (h, l) = (env.high[i] >> pin & 1 != 0, env.low[i] >> pin & 1 != 0);
        let (s, e) = (env.start[i], env.end[i]);
        if h && l {
            points.extend([[s, low], [s, high], [e, high], [e, low]]);
        } else {
            let y = if h { high } else { low };
            points.extend([[s, y], [e, y]]);
        }
    }
    PlotPoints::new(points)
}

fn to_y(log: bool, micro_amps: f64) -> f64 {
    if log { micro_amps.max(LOG_FLOOR_UA).log10() } else { micro_amps }
}

fn from_y(log: bool, y: f64) -> f64 {
    if log { 10f64.powf(y) } else { y }
}

/// The smallest 1, 2 or 5 times a power of ten that is at least `x` (> 0)
fn nice_ceil(x: f64) -> f64 {
    let e = 10f64.powf(x.log10().floor());
    [1., 2., 5., 10.].into_iter().map(|m| m * e).find(|&v| v >= x * (1. - 1e-9)).unwrap_or(10. * e)
}

/// Grid with the statistics of a [Report]. With `interval`, its (wall clock) duration is shown as well
fn report_grid(ui: &mut egui::Ui, id: &str, r: &Report, interval: bool) {
    egui::Grid::new(id).num_columns(2).spacing([16., 4.]).show(ui, |ui| {
        let mut row = |name: &str, value: String| {
            ui.label(theme::title(name));
            ui.label(RichText::new(value).color(theme::p().text));
            ui.end_row();
        };
        row("avg", format_current(r.avg_ua));
        row("rms", format_current(r.rms_ua));
        row("min", format_current(r.min_ua.into()));
        row("max", format_current(r.max_ua.into()));
        row("charge", format_charge(r.charge_uc));
        if let Some((mv, uw)) = r.power {
            row("avg power", format!("{} at {mv} mV", format_power(uw)));
        }
        if interval {
            row("samples", format!("{} in {:.3} s", r.samples, r.duration_s));
        } else {
            row("samples", format!("{}", r.samples));
        }
        if let Some(s) = &r.split {
            row("split at", format_current(s.threshold_ua.into()));
            row("active", format!("{:.2} %, avg {}", s.active_pct, format_current(s.active_avg_ua)));
            row("", format!("{:.1} % of charge", s.active_charge_pct));
            row("bursts", format!("{} of {:.3} ms avg", s.bursts, s.burst_ms));
            row("sleep", format!("{:.2} %, avg {}", s.sleep_pct, format_current(s.sleep_avg_ua)));
        }
    });
}

/// Height of [App::range_stats], with the split and the digital condition
fn range_stats_height(ui: &egui::Ui, split: bool, condition: bool) -> f32 {
    // Card margins, the title row and the rows of values
    let row = ui.text_style_height(&egui::TextStyle::Body) + 4.;
    let rows = if split { 3. } else { 2. };
    let condition = if condition { 24. + 32. + row + ui.spacing().item_spacing.y * 2. } else { 0. };
    24. + 32. + rows * row + 4. + ui.spacing().item_spacing.y * 3. + condition
}

/// The statistics of a [Report] in a few rows of name/value columns, for under the plots
fn compact_report(ui: &mut egui::Ui, id: &str, r: &Report) {
    egui::Grid::new(id).num_columns(6).spacing([10., 4.]).show(ui, |ui| {
        report_cell(ui, "avg", format_current(r.avg_ua));
        report_cell(ui, "min", format_current(r.min_ua.into()));
        report_cell(ui, "charge", format_charge_short(r.charge_uc));
        ui.end_row();
        report_cell(ui, "rms", format_current(r.rms_ua));
        report_cell(ui, "max", format_current(r.max_ua.into()));
        match r.power {
            Some((_, uw)) => report_cell(ui, "power", format_power(uw)),
            None => report_cell(ui, "samples", format!("{}", r.samples)),
        }
        ui.end_row();
        if let Some(s) = &r.split {
            report_cell(ui, "active", format!("{:.2} %, {}", s.active_pct, format_current(s.active_avg_ua)));
            report_cell(ui, "sleep", format!("{:.2} %, {}", s.sleep_pct, format_current(s.sleep_avg_ua)));
            report_cell(ui, "bursts", format!("{} of {:.3} ms, {:.1} % of charge", s.bursts, s.burst_ms, s.active_charge_pct));
            ui.end_row();
        }
    });
}

/// A column that scrolls (also by dragging) when it doesn't fit
fn column_scroll(ui: &mut egui::Ui, id: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink(false)
        .scroll_source(ScrollSource::ALL)
        .show(ui, add_contents);
}

/// A name and its value, as two grid cells
fn report_cell(ui: &mut egui::Ui, name: &str, value: String) {
    ui.label(theme::title(name));
    ui.label(RichText::new(value).color(theme::p().text));
}

/// A collapsible section of the statistics
fn section(ui: &mut egui::Ui, name: &str, open: bool, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(theme::value(name, 15.))
        .default_open(open)
        .show(ui, add_contents);
}

/// Recorded time as h:mm:ss.s, or seconds while short
fn format_recorded(s: f64) -> String {
    if s < 60. {
        format!("{s:.1} s")
    } else {
        let t = s as u64;
        format!("{}:{:02}:{:04.1}", t / 3600, t / 60 % 60, s % 60.)
    }
}

/// A time span with 3 significant digits, e.g. "3.23 s" or "12.5 ms"
fn format_seconds(s: f64) -> String {
    let (v, unit) = if s < 1. { (s * 1000., "ms") } else { (s, "s") };
    let decimals = if v >= 100. { 0 } else if v >= 10. { 1 } else { 2 };
    let v = format!("{v:.decimals$}");
    let v = if v.contains('.') { v.trim_end_matches('0').trim_end_matches('.') } else { &v };
    format!("{v} {unit}")
}

/// A buffer length, e.g. "5 min" or "8 h"
fn format_buffer(s: f64) -> String {
    if s >= 3600. {
        format!("{} h", s / 3600.)
    } else {
        format!("{} min", s / 60.)
    }
}

/// The storage rate when combining `samples_per_stored` samples, e.g. "100 kS/s"
fn format_rate(samples_per_stored: u64) -> String {
    let rate = SAMPLE_RATE / samples_per_stored as f64;
    if rate >= 1000. {
        format!("{} kS/s", rate / 1000.)
    } else {
        format!("{rate} S/s")
    }
}

/// A memory size, e.g. "198 MB" or "2.4 GB"
fn format_bytes(bytes: f64) -> String {
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.0} MB", bytes / 1e6)
    }
}

fn format_duration(s: f64) -> String {
    if s < 1e-3 {
        format!("{:.0} µs", s * 1e6)
    } else if s < 1. {
        format!("{:.3} ms", s * 1e3)
    } else {
        format!("{s:.4} s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_seconds;

    #[test]
    fn seconds() {
        assert_eq!(format_seconds(3.232148626087143), "3.23 s");
        assert_eq!(format_seconds(5.), "5 s");
        assert_eq!(format_seconds(60.), "60 s");
        assert_eq!(format_seconds(0.5), "500 ms");
        assert_eq!(format_seconds(0.0125), "12.5 ms");
        assert_eq!(format_seconds(0.001234), "1.23 ms");
        assert_eq!(format_seconds(123.456), "123 s");
    }
}
