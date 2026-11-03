//! Storage of the measured samples: the stored samples (at full or reduced rate), min/max/average buckets for
//! plotting, plus the statistics

use std::{collections::VecDeque, mem::size_of};

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::stats::{Report, Stats};

/// The PPK2 always samples at 100 kS/s
pub const SAMPLE_RATE: f64 = 100_000.;

/// Number of interval reports kept
const MAX_REPORTS: usize = 100;
/// The plot levels combine this many and its square of stored samples into a bucket
const LEVEL_FACTOR: u64 = 10;

#[derive(Clone, Copy)]
struct Bucket {
    min: f32,
    max: f32,
    sum: f32,
    /// Logic port pins that were high (bit set) or low (bit set) during the bucket
    high: u8,
    low: u8,
}

impl Bucket {
    const EMPTY: Self = Self { min: f32::INFINITY, max: f32::NEG_INFINITY, sum: 0., high: 0, low: 0 };

    fn sample(v: f32, pins: u8) -> Self {
        Self { min: v, max: v, sum: v, high: pins, low: !pins }
    }

    fn merge(&mut self, other: &Bucket) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
        self.sum += other.sum;
        self.high |= other.high;
        self.low |= other.low;
    }

    /// Whether every sample of the bucket meets `condition`
    fn meets(&self, condition: PinCondition) -> bool {
        self.high & condition.care & !condition.high == 0 && self.low & condition.care & condition.high == 0
    }
}

/// The samples combined into buckets of a fixed number of samples: one level of detail
struct Level {
    samples_per_bucket: u64,
    buckets: VecDeque<Bucket>,
    /// Index (since the start of the recording) of the first bucket in `buckets`
    first: u64,
    pending: Bucket,
    pending_count: u64,
    max_buckets: usize,
}

impl Level {
    fn new(samples_per_bucket: u64, max_seconds: f64) -> Self {
        Self {
            samples_per_bucket,
            buckets: VecDeque::new(),
            first: 0,
            pending: Bucket::EMPTY,
            pending_count: 0,
            max_buckets: max_buckets(samples_per_bucket, max_seconds),
        }
    }

    fn set_max_seconds(&mut self, max_seconds: f64) {
        self.max_buckets = max_buckets(self.samples_per_bucket, max_seconds);
        let excess = self.buckets.len().saturating_sub(self.max_buckets);
        self.buckets.drain(..excess);
        self.first += excess as u64;
    }

    fn push(&mut self, v: f32, pins: u8) {
        self.pending.merge(&Bucket::sample(v, pins));
        self.pending_count += 1;
        if self.pending_count == self.samples_per_bucket {
            if self.buckets.len() == self.max_buckets {
                self.buckets.pop_front();
                self.first += 1;
            }
            self.buckets.push_back(self.pending);
            self.pending = Bucket::EMPTY;
            self.pending_count = 0;
        }
    }

    fn envelope(&self, k: std::ops::Range<u64>, samples_per_column: f64, out: &mut Envelope) {
        let end = self.first + self.buckets.len() as u64;
        envelope(self.first, end, self.samples_per_bucket, k, samples_per_column, out, |i| {
            self.buckets[(i - self.first) as usize]
        });
    }
}

fn max_buckets(samples_per_bucket: u64, max_seconds: f64) -> usize {
    (max_seconds * SAMPLE_RATE / samples_per_bucket as f64) as usize
}

/// The stored samples: at the full rate the samples themselves (5 bytes each), else buckets of
/// `samples_per_stored` samples that keep their minimum, maximum and sum
struct Store {
    samples_per_stored: u64,
    /// At the full rate: the samples (µA) and their logic port pins
    values: VecDeque<f32>,
    pins: VecDeque<u8>,
    /// At a reduced rate
    buckets: Level,
}

impl Store {
    fn new(samples_per_stored: u64, max_seconds: f64) -> Self {
        Self {
            samples_per_stored,
            values: VecDeque::new(),
            pins: VecDeque::new(),
            buckets: Level::new(samples_per_stored, max_seconds),
        }
    }

    fn full_rate(&self) -> bool {
        self.samples_per_stored == 1
    }

    fn set_max_seconds(&mut self, max_seconds: f64) {
        self.buckets.set_max_seconds(max_seconds);
        if self.full_rate() {
            // The level holds no buckets at the full rate, but its index tracks the dropped samples
            let excess = self.values.len().saturating_sub(self.buckets.max_buckets);
            self.values.drain(..excess);
            self.pins.drain(..excess);
            self.buckets.first += excess as u64;
        }
    }

    fn push(&mut self, v: f32, pins: u8) {
        if self.full_rate() {
            if self.values.len() == self.buckets.max_buckets {
                self.values.pop_front();
                self.pins.pop_front();
                self.buckets.first += 1;
            }
            self.values.push_back(v);
            self.pins.push_back(pins);
        } else {
            self.buckets.push(v, pins);
        }
    }

    /// Index (since the start of the recording) of the first stored sample
    fn first(&self) -> u64 {
        self.buckets.first
    }

    fn len(&self) -> usize {
        if self.full_rate() {
            self.values.len()
        } else {
            self.buckets.buckets.len()
        }
    }

    /// The stored sample with index `i`, since the start of the recording
    fn get(&self, i: u64) -> Bucket {
        let i = (i - self.first()) as usize;
        if self.full_rate() {
            Bucket::sample(self.values[i], self.pins[i])
        } else {
            self.buckets.buckets[i]
        }
    }

    fn envelope(&self, k: std::ops::Range<u64>, samples_per_column: f64, out: &mut Envelope) {
        let first = self.first();
        envelope(first, first + self.len() as u64, self.samples_per_stored, k, samples_per_column, out, |i| {
            self.get(i)
        });
    }
}

/// Min/max/average of the columns `k` of `samples_per_column` samples each, from the buckets `first..end` of
/// `samples_per_bucket` samples, as returned by `get`. The columns are aligned to the start of the recording,
/// so when the view moves by whole columns every column keeps its content: no aliasing jitter.
fn envelope(
    first: u64,
    end: u64,
    samples_per_bucket: u64,
    k: std::ops::Range<u64>,
    samples_per_column: f64,
    out: &mut Envelope,
    get: impl Fn(u64) -> Bucket,
) {
    let spb = samples_per_bucket as f64;
    for k in k {
        let b0 = ((k as f64 * samples_per_column / spb).floor() as u64).max(first);
        let b1 = (((k + 1) as f64 * samples_per_column / spb).floor() as u64).min(end);
        if b1 <= b0 {
            continue;
        }
        let mut b = Bucket::EMPTY;
        for i in b0..b1 {
            b.merge(&get(i));
        }
        let n = (b1 - b0) as f64;
        let ts = b0 as f64 * spb / SAMPLE_RATE;
        let te = b1 as f64 * spb / SAMPLE_RATE;
        out.start.push(ts);
        out.end.push(te);
        out.t.push((ts + te) / 2.);
        out.min.push(b.min.into());
        out.max.push(b.max.into());
        out.avg.push(f64::from(b.sum) / (n * spb));
        out.high.push(b.high);
        out.low.push(b.low);
    }
}

/// A condition on the logic port pins: every pin in `care` at the level in `high`
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PinCondition {
    pub care: u8,
    pub high: u8,
}

impl PinCondition {
    /// Whether the condition is on any pin at all
    pub fn is_set(&self) -> bool {
        self.care != 0
    }

    /// The level required for `pin`: `None` for either
    pub fn level(&self, pin: usize) -> Option<bool> {
        (self.care >> pin & 1 != 0).then_some(self.high >> pin & 1 != 0)
    }

    pub fn set_level(&mut self, pin: usize, level: Option<bool>) {
        let bit = 1 << pin;
        self.care = (self.care & !bit) | if level.is_some() { bit } else { 0 };
        self.high = (self.high & !bit) | if level == Some(true) { bit } else { 0 };
    }
}

/// Statistics of the samples meeting a [PinCondition]
pub struct ConditionReport {
    /// Statistics of the matching samples, if any
    pub report: Option<Report>,
    /// Fraction (0..1) of the samples that match
    pub fraction: f64,
    /// Number of times the condition started to hold
    pub occurrences: u64,
}

/// Plot data: time (s), min/max/average current (µA) and logic port state of every point
#[derive(Default)]
pub struct Envelope {
    /// Middle, start and end time
    pub t: Vec<f64>,
    pub start: Vec<f64>,
    pub end: Vec<f64>,
    pub min: Vec<f64>,
    pub max: Vec<f64>,
    pub avg: Vec<f64>,
    /// Logic port pins that were high (bit set) or low (bit set) during the point
    pub high: Vec<u8>,
    pub low: Vec<u8>,
}

/// The latest samples, also while not recording: for the readout and the meter
pub struct Live {
    values: VecDeque<f32>,
    max_len: usize,
}

impl Live {
    /// Keeping the last `seconds`
    pub fn new(seconds: f64) -> Self {
        Self { values: VecDeque::new(), max_len: (seconds * SAMPLE_RATE) as usize }
    }

    pub fn push(&mut self, micro_amps: f32) {
        if self.values.len() == self.max_len {
            self.values.pop_front();
        }
        self.values.push_back(micro_amps);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// Average, minimum and maximum current (µA) over the last `seconds`
    pub fn recent(&self, seconds: f64) -> Option<(f64, f64, f64)> {
        let n = ((seconds * SAMPLE_RATE) as usize).max(1);
        let (mut sum, mut count, mut min, mut max) = (0f64, 0usize, f32::INFINITY, f32::NEG_INFINITY);
        for &v in self.values.iter().rev().take(n) {
            sum += f64::from(v);
            count += 1;
            min = min.min(v);
            max = max.max(v);
        }
        (count > 0).then(|| (sum / count as f64, min.into(), max.into()))
    }
}

/// Memory (bytes) a recording takes per second, storing one value per `samples_per_stored` samples
pub fn bytes_per_second(samples_per_stored: u64) -> f64 {
    let stored = if samples_per_stored == 1 { size_of::<f32>() + size_of::<u8>() } else { size_of::<Bucket>() };
    let n = samples_per_stored as f64;
    let levels = size_of::<Bucket>() as f64 * (1. / LEVEL_FACTOR as f64 + 1. / (LEVEL_FACTOR * LEVEL_FACTOR) as f64);
    SAMPLE_RATE / n * (stored as f64 + levels)
}

/// A recording of samples
pub struct Recording {
    /// The stored samples, for the statistics of a selection and plotting zoomed in
    store: Store,
    /// Buckets of [LEVEL_FACTOR] and its square times as many samples, for plotting zoomed out
    mid: Level,
    coarse: Level,
    max_seconds: f64,
    /// Number of samples since the start of the recording
    pub samples: u64,
    /// Changes on every [Recording::clear], to invalidate anything computed from the samples
    pub generation: u64,
    /// Source voltage, in source meter mode only
    pub source_mv: Option<u16>,
    /// Statistics interval, counted in samples
    pub interval_ms: u64,
    /// Split the statistics into active (at or above this current in µA) and sleep parts
    pub split_ua: Option<f32>,
    interval: Stats,
    interval_start: Option<Instant>,
    /// Statistics of the last intervals, most recent last
    pub reports: VecDeque<Report>,
    totals: Stats,
}

impl Recording {
    /// A recording keeping the last `max_seconds`, storing one value per `samples_per_stored` samples
    pub fn new(max_seconds: f64, samples_per_stored: u64) -> Self {
        let n = samples_per_stored.max(1);
        Self {
            store: Store::new(n, max_seconds),
            mid: Level::new(n * LEVEL_FACTOR, max_seconds),
            coarse: Level::new(n * LEVEL_FACTOR * LEVEL_FACTOR, max_seconds),
            max_seconds,
            samples: 0,
            generation: 0,
            source_mv: None,
            interval_ms: 1000,
            split_ua: None,
            interval: Stats::default(),
            interval_start: None,
            reports: VecDeque::new(),
            totals: Stats::default(),
        }
    }

    pub fn max_seconds(&self) -> f64 {
        self.max_seconds
    }

    pub fn samples_per_stored(&self) -> u64 {
        self.store.samples_per_stored
    }

    /// Keep the last `max_seconds` from now on, dropping older samples
    pub fn set_max_seconds(&mut self, max_seconds: f64) {
        self.max_seconds = max_seconds;
        self.store.set_max_seconds(max_seconds);
        self.mid.set_max_seconds(max_seconds);
        self.coarse.set_max_seconds(max_seconds);
        self.generation += 1;
    }

    /// Store one value per `samples_per_stored` samples from now on. Clears the recording.
    pub fn set_samples_per_stored(&mut self, samples_per_stored: u64) {
        let source_mv = self.source_mv;
        let (interval_ms, split_ua, generation) = (self.interval_ms, self.split_ua, self.generation);
        *self = Self::new(self.max_seconds, samples_per_stored);
        self.source_mv = source_mv;
        self.interval_ms = interval_ms;
        self.split_ua = split_ua;
        self.generation = generation + 1;
    }

    pub fn clear(&mut self) {
        let generation = self.generation;
        self.set_samples_per_stored(self.store.samples_per_stored);
        self.generation = generation + 1;
    }

    /// Continue after a pause: the running interval would include the pause, so start a new one
    pub fn resume(&mut self) {
        self.interval = self.interval.next();
        self.interval_start = None;
    }

    /// Add a sample (µA) and its logic port pins
    pub fn push(&mut self, micro_amps: f32, pins: u8) {
        self.store.push(micro_amps, pins);
        self.mid.push(micro_amps, pins);
        self.coarse.push(micro_amps, pins);
        self.samples += 1;
        self.totals.add(micro_amps, self.split_ua);

        // The interval is counted in samples, so USB buffering doesn't skew it. Its (wall clock) duration is
        // reported too: a clearly longer duration than the interval means samples were missed.
        let start = *self.interval_start.get_or_insert_with(Instant::now);
        self.interval.add(micro_amps, self.split_ua);
        let samples_per_interval = (SAMPLE_RATE as u64 * self.interval_ms / 1000).max(1);
        if self.interval.count >= samples_per_interval {
            let duration_s = start.elapsed().as_secs_f64();
            if self.reports.len() == MAX_REPORTS {
                self.reports.pop_front();
            }
            self.reports
                .push_back(Report::new(&self.interval, duration_s, self.source_mv, self.split_ua));
            self.interval = self.interval.next();
            self.interval_start = Some(Instant::now());
        }
    }

    /// Recorded time (s)
    pub fn duration_s(&self) -> f64 {
        self.samples as f64 / SAMPLE_RATE
    }

    /// Start of the time still available (s): older samples have been dropped
    pub fn start_s(&self) -> f64 {
        (self.store.first() * self.store.samples_per_stored) as f64 / SAMPLE_RATE
    }

    /// Statistics over the whole recording
    pub fn totals(&self) -> Option<Report> {
        (self.totals.count > 0)
            .then(|| Report::new(&self.totals, self.duration_s(), self.source_mv, self.split_ua))
    }

    /// Add a stored sample to `stats`. At a reduced rate, the rms and the split use the average of its samples.
    fn add_stored(&self, stats: &mut Stats, b: &Bucket) {
        stats.add_samples(self.store.samples_per_stored, f64::from(b.sum), b.min, b.max, self.split_ua);
    }

    /// Statistics of the time range `t0..t1` (s)
    pub fn range_report(&self, t0: f64, t1: f64) -> Option<Report> {
        let mut stats = Stats::default();
        for i in self.stored_range(t0, t1) {
            self.add_stored(&mut stats, &self.store.get(i));
        }
        (stats.count > 0).then(|| Report::new(&stats, stats.count as f64 / SAMPLE_RATE, self.source_mv, self.split_ua))
    }

    /// Statistics of the samples in the time range `t0..t1` (s) that meet `condition`. At a reduced rate, a
    /// stored sample meets it when all of its samples do.
    pub fn condition_report(&self, t0: f64, t1: f64, condition: PinCondition) -> ConditionReport {
        let range = self.stored_range(t0, t1);
        let total = (range.end - range.start) as f64;
        let mut stats = Stats::default();
        let (mut matched, mut occurrences, mut previous) = (0u64, 0, false);
        for i in range {
            let b = self.store.get(i);
            let meets = b.meets(condition);
            if meets {
                self.add_stored(&mut stats, &b);
                matched += 1;
                if !previous {
                    occurrences += 1;
                }
            }
            previous = meets;
        }
        ConditionReport {
            report: (stats.count > 0)
                .then(|| Report::new(&stats, stats.count as f64 / SAMPLE_RATE, self.source_mv, self.split_ua)),
            fraction: if total > 0. { matched as f64 / total } else { 0. },
            occurrences,
        }
    }

    /// Indices (since the start of the recording) of the stored samples in the time range `t0..t1` (s)
    fn stored_range(&self, t0: f64, t1: f64) -> std::ops::Range<u64> {
        let first = self.store.first();
        let end = first + self.store.len() as u64;
        let index = |t: f64| ((t * SAMPLE_RATE / self.store.samples_per_stored as f64).max(0.) as u64).clamp(first, end);
        index(t0)..index(t1)
    }

    /// Average, minimum and maximum current (µA) over the last `seconds`
    pub fn recent(&self, seconds: f64) -> Option<(f64, f64, f64)> {
        let n = self.store.samples_per_stored;
        let count = ((seconds * SAMPLE_RATE / n as f64).ceil() as u64).clamp(1, self.store.len() as u64);
        let end = self.store.first() + self.store.len() as u64;
        let mut b = Bucket::EMPTY;
        for i in end - count..end {
            b.merge(&self.store.get(i));
        }
        (self.store.len() > 0).then(|| (f64::from(b.sum) / (count * n) as f64, b.min.into(), b.max.into()))
    }

    /// Min/max/average of the time range `t0..t1` (s) in `columns` points, for a plot `columns` pixels wide.
    /// Each point covers the same time range, aligned to the start of the recording.
    pub fn envelope(&self, t0: f64, t1: f64, columns: usize) -> Envelope {
        let mut out = Envelope::default();
        // Zoomed in further than the stored samples: a point per stored sample
        let w = ((t1 - t0) * SAMPLE_RATE / columns.max(1) as f64).max(self.store.samples_per_stored as f64);
        let k = (t0 * SAMPLE_RATE / w).floor().max(0.) as u64..(t1 * SAMPLE_RATE / w).ceil().max(0.) as u64;
        // The coarsest level with at most a bucket per point
        if w >= self.coarse.samples_per_bucket as f64 {
            self.coarse.envelope(k, w, &mut out);
        } else if w >= self.mid.samples_per_bucket as f64 {
            self.mid.envelope(k, w, &mut out);
        } else {
            self.store.envelope(k, w, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{PinCondition, Recording};

    #[test]
    fn condition_report() {
        let mut rec = Recording::new(20., 1);
        crate::screenshot::demo_samples(&mut rec);
        let mut d0 = PinCondition::default();
        d0.set_level(0, Some(true));
        let c = rec.condition_report(0., 12., d0);
        assert_eq!(c.occurrences, 12);
        assert!((c.fraction - 0.045).abs() < 1e-3, "{}", c.fraction);
        let r = c.report.unwrap();
        // 3 ms at about 6 mA and 42 ms at about 650 µA
        assert!((r.avg_ua - (3. * 6000. + 42. * 650.) / 45.).abs() < 20., "{}", r.avg_ua);

        // D0 high and D1 low: the D1 toggling during the first 3 ms of each burst splits those up
        let mut d0_not_d1 = d0;
        d0_not_d1.set_level(1, Some(false));
        assert!(rec.condition_report(0., 12., d0_not_d1).occurrences > 12);
        assert_eq!(d0_not_d1.level(1), Some(false));
        assert_eq!(d0_not_d1.level(2), None);
    }

    #[test]
    fn reduced_rate() {
        let mut full = Recording::new(20., 1);
        crate::screenshot::demo_samples(&mut full);
        let mut reduced = Recording::new(20., 100);
        crate::screenshot::demo_samples(&mut reduced);

        // Average, minimum, maximum and charge stay exact
        let (f, r) = (full.range_report(1., 11.).unwrap(), reduced.range_report(1., 11.).unwrap());
        assert_eq!(f.samples, r.samples);
        assert!((f.avg_ua - r.avg_ua).abs() < 1e-3 * f.avg_ua, "{} {}", f.avg_ua, r.avg_ua);
        assert_eq!((f.min_ua, f.max_ua), (r.min_ua, r.max_ua));
        assert_eq!(full.start_s(), reduced.start_s());
        assert_eq!(full.recent(0.5).map(|r| r.2), reduced.recent(0.5).map(|r| r.2));

        // D0 is high for 45 ms per second: 4.5 ms (rounding to the 1 ms stored samples) at most
        let mut d0 = PinCondition::default();
        d0.set_level(0, Some(true));
        let c = reduced.condition_report(0., 12., d0);
        assert_eq!(c.occurrences, 12);
        assert!((c.fraction - 0.045).abs() < 2e-3, "{}", c.fraction);
    }

    #[test]
    fn max_seconds() {
        let mut rec = Recording::new(20., 10);
        crate::screenshot::demo_samples(&mut rec);
        assert_eq!(rec.start_s(), 0.);
        rec.set_max_seconds(5.);
        assert!((rec.start_s() - 7.).abs() < 1e-9, "{}", rec.start_s());
        assert!(rec.range_report(0., 7.).is_none());
        assert!(rec.range_report(7., 12.).is_some());
        // A reduced rate clears the recording
        rec.set_samples_per_stored(100);
        assert_eq!(rec.samples, 0);
        assert_eq!(rec.max_seconds(), 5.);
    }
}
