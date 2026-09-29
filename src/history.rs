//! Bounded time-series storage used by the live graphs.
//!
//! A [`Series`] is a fixed capacity ring buffer. Pushing past the capacity evicts the
//! oldest sample, so the memory used by the graphs is constant no matter how long the
//! application runs.

use std::collections::VecDeque;

/// Minimum number of samples kept in a series.
const MIN_CAPACITY: usize = 8;

/// Maximum number of samples a single series may hold.
///
/// This is a hard safety net; the configured history length is expected to be well
/// below it.
const MAX_CAPACITY: usize = 4096;

/// A bounded ring buffer of `f64` samples with graph-oriented accessors.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    values: VecDeque<f64>,
    capacity: usize,
}

impl Series {
    /// Creates a series with room for `capacity` samples (clamped to 8..=4096).
    pub fn new(capacity: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(capacity.min(MAX_CAPACITY)),
            capacity: capacity.clamp(MIN_CAPACITY, MAX_CAPACITY),
        }
    }

    /// Number of samples currently stored.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the series has no samples yet.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Maximum number of samples the series can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Appends a sample, evicting the oldest one when the series is full.
    pub fn push(&mut self, value: f64) {
        if !value.is_finite() {
            // Never let a NaN produced by a divide-by-zero reach the widgets: they
            // would render garbage or panic depending on the backend.
            return;
        }
        if self.values.len() == self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    /// Iterates over the samples from oldest to newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = f64> + ExactSizeIterator + '_ {
        self.values.iter().copied()
    }

    /// Returns the most recent sample, if any.
    pub fn last(&self) -> Option<f64> {
        self.values.back().copied()
    }

    /// Smallest sample in the buffer, or `0.0` when empty.
    pub fn min(&self) -> f64 {
        self.extreme(f64::min).unwrap_or(0.0)
    }

    /// Largest sample in the buffer, or `0.0` when empty.
    pub fn max(&self) -> f64 {
        self.extreme(f64::max).unwrap_or(0.0)
    }

    /// Folds the buffer with `op`, returning `None` for an empty series.
    fn extreme(&self, op: fn(f64, f64) -> f64) -> Option<f64> {
        self.values.iter().copied().reduce(op)
    }

    /// Drops all samples, keeping the configured capacity.
    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// Resizes the ring buffer, trimming the oldest samples when shrinking.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.clamp(MIN_CAPACITY, MAX_CAPACITY);
        while self.values.len() > self.capacity {
            self.values.pop_front();
        }
    }

    /// Returns samples as `(index, value)` pairs, ready for a `Chart` dataset.
    ///
    /// The index is the position of the sample counting from the oldest retained
    /// sample, so graphs scroll to the right as new data arrives.
    pub fn chart_points(&self) -> Vec<(f64, f64)> {
        self.values
            .iter()
            .copied()
            .enumerate()
            .map(|(i, v)| (i as f64, v))
            .collect()
    }

    /// Returns samples scaled into the `u64` domain expected by `Sparkline`.
    ///
    /// Values are scaled so the largest sample maps to `max`, which keeps the
    /// relative shape of the graph readable regardless of the unit being plotted.
    pub fn spark_values(&self, max: u64) -> Vec<u64> {
        let peak = self.max();
        if peak <= 0.0 {
            return vec![0; self.values.len()];
        }
        self.values
            .iter()
            .map(|v| ((v / peak) * max as f64).round().clamp(0.0, max as f64) as u64)
            .collect()
    }
}

impl Default for Series {
    fn default() -> Self {
        Self::new(120)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_clamped_to_supported_range() {
        assert_eq!(Series::new(0).capacity(), MIN_CAPACITY);
        assert_eq!(Series::new(1_000_000).capacity(), MAX_CAPACITY);
        assert_eq!(Series::new(64).capacity(), 64);
    }

    #[test]
    fn push_evicts_oldest_when_full() {
        let mut series = Series::new(MIN_CAPACITY);
        for value in 0..(MIN_CAPACITY + 10) {
            series.push(value as f64);
        }
        assert_eq!(series.len(), MIN_CAPACITY);
        assert_eq!(series.last(), Some(MIN_CAPACITY as f64 + 9.0));
        assert_eq!(series.iter().next(), Some(10.0));
    }

    #[test]
    fn non_finite_samples_are_ignored() {
        let mut series = Series::new(16);
        series.push(f64::NAN);
        series.push(f64::INFINITY);
        series.push(1.0);
        assert_eq!(series.len(), 1);
        assert_eq!(series.last(), Some(1.0));
    }

    #[test]
    fn shrink_trims_oldest_samples() {
        let mut series = Series::new(32);
        for value in 0..10 {
            series.push(f64::from(value));
        }
        series.set_capacity(8);
        assert_eq!(series.len(), 8);
        assert_eq!(series.iter().next(), Some(2.0));
    }

    #[test]
    fn min_max_handle_empty_series() {
        let series = Series::new(16);
        assert_eq!(series.min(), 0.0);
        assert_eq!(series.max(), 0.0);
        assert!(series.is_empty());
    }

    #[test]
    fn min_max_report_raw_extremes() {
        let mut series = Series::new(16);
        series.push(2.0);
        series.push(-5.0);
        series.push(9.0);
        assert_eq!(series.min(), -5.0);
        assert_eq!(series.max(), 9.0);
    }

    #[test]
    fn chart_points_are_indexed_from_zero() {
        let mut series = Series::new(16);
        series.push(3.0);
        series.push(4.0);
        assert_eq!(series.chart_points(), vec![(0.0, 3.0), (1.0, 4.0)]);
    }

    #[test]
    fn spark_values_scale_to_max() {
        let mut series = Series::new(16);
        series.push(0.0);
        series.push(50.0);
        series.push(100.0);
        assert_eq!(series.spark_values(8), vec![0, 4, 8]);
    }

    #[test]
    fn spark_values_of_flat_series_fill_the_bar() {
        let mut series = Series::new(16);
        series.push(7.0);
        series.push(7.0);
        assert_eq!(series.spark_values(4), vec![4, 4]);
    }
}
