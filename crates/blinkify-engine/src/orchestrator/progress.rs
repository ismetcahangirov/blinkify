//! Parsing `-progress pipe:1`, and deciding what reaches the renderer.
//!
//! FFmpeg writes a block of `key=value` lines roughly twice a second, ending
//! each with `progress=continue`, and the last with `progress=end`. That format
//! is designed for machines and is stable across versions, unlike the status
//! line on stderr.
//!
//! Two promises are kept here rather than left to each caller:
//!
//! - **100 percent is reported exactly once**, and only for a job that
//!   succeeded. Mid-run values are clamped below it, so a job that reaches the
//!   end of its input and then fails while finalising never showed "done".
//! - **Updates are throttled.** Every event crosses the Tauri IPC boundary and
//!   repaints something; a hundred a second is a performance bug, and one a
//!   second looks frozen.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The most often a progress update is forwarded.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// The largest fraction reported before the job has actually finished.
const BELOW_DONE: f64 = 0.999;

/// Progress of one job, as the renderer receives it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Progress {
    /// From 0 to 1. Exactly 1 only in the single final update of a job that
    /// succeeded.
    pub fraction: f64,
    /// Encoding speed relative to real time, when FFmpeg reports one.
    pub speed: Option<f64>,
}

/// One parsed `-progress` block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressBlock {
    /// Media time processed so far.
    pub out_time: Option<Duration>,
    pub speed: Option<f64>,
    /// `progress=end` was seen.
    pub end: bool,
}

/// Accumulates `-progress` lines into blocks.
#[derive(Debug, Default)]
pub struct ProgressParser {
    out_time: Option<Duration>,
    speed: Option<f64>,
}

impl ProgressParser {
    /// Feed one line. Returns a block when the line closes one.
    pub fn line(&mut self, line: &str) -> Option<ProgressBlock> {
        let (key, value) = line.trim().split_once('=')?;
        match key {
            // Despite the name, `out_time_ms` is also in microseconds; prefer
            // the key that says so. Negative values appear before the first
            // packet and mean "nothing yet".
            "out_time_us" | "out_time_ms" => {
                if let Ok(micros) = value.trim().parse::<i64>() {
                    self.out_time = u64::try_from(micros).ok().map(Duration::from_micros);
                }
            }
            "speed" => {
                self.speed = value
                    .trim()
                    .trim_end_matches('x')
                    .parse::<f64>()
                    .ok()
                    .filter(|speed| speed.is_finite());
            }
            "progress" => {
                return Some(ProgressBlock {
                    out_time: self.out_time,
                    speed: self.speed,
                    end: value.trim() == "end",
                });
            }
            _ => {}
        }
        None
    }
}

/// Turns blocks into the updates the renderer sees.
#[derive(Debug)]
pub struct ProgressReporter {
    total: Duration,
    last_sent: Option<Instant>,
    last_fraction: f64,
    finished: bool,
}

impl ProgressReporter {
    #[must_use]
    pub fn new(total: Duration) -> Self {
        Self {
            total,
            last_sent: None,
            last_fraction: 0.0,
            finished: false,
        }
    }

    /// The update to forward for a mid-run block, if one is due.
    ///
    /// Never 1.0 — see [`ProgressReporter::finish`]. Never backwards either:
    /// FFmpeg's `out_time` can dip when a filter graph reorders, and a bar that
    /// jumps back reads as a bug.
    pub fn block(&mut self, block: &ProgressBlock, now: Instant) -> Option<Progress> {
        if self.finished {
            return None;
        }
        let fraction = self.fraction(block.out_time).max(self.last_fraction);
        let due = self
            .last_sent
            .is_none_or(|last| now.saturating_duration_since(last) >= MIN_INTERVAL);
        if !due || fraction <= self.last_fraction && self.last_sent.is_some() {
            return None;
        }
        self.last_sent = Some(now);
        self.last_fraction = fraction;
        Some(Progress {
            fraction,
            speed: block.speed,
        })
    }

    /// The final update of a job that succeeded: exactly 1.0, exactly once.
    pub fn finish(&mut self) -> Option<Progress> {
        if self.finished {
            return None;
        }
        self.finished = true;
        self.last_fraction = 1.0;
        Some(Progress {
            fraction: 1.0,
            speed: None,
        })
    }

    fn fraction(&self, out_time: Option<Duration>) -> f64 {
        let Some(out_time) = out_time else {
            return 0.0;
        };
        if self.total.is_zero() {
            return 0.0;
        }
        (out_time.as_secs_f64() / self.total.as_secs_f64()).clamp(0.0, BELOW_DONE)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    fn feed(parser: &mut ProgressParser, text: &str) -> Vec<ProgressBlock> {
        text.lines().filter_map(|line| parser.line(line)).collect()
    }

    #[test]
    fn a_block_ends_at_the_progress_key() {
        let mut parser = ProgressParser::default();
        let blocks = feed(
            &mut parser,
            "frame=10\nout_time_us=2500000\nspeed=2.5x\nprogress=continue\nout_time_us=5000000\nprogress=end\n",
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].out_time, Some(Duration::from_millis(2500)));
        assert_eq!(blocks[0].speed, Some(2.5));
        assert!(!blocks[0].end);
        assert!(blocks[1].end);
    }

    #[test]
    fn negative_and_na_times_mean_nothing_yet() {
        let mut parser = ProgressParser::default();
        let blocks = feed(
            &mut parser,
            "out_time_us=-9223372036854775807\nspeed=N/A\nprogress=continue\n",
        );
        assert_eq!(blocks[0].out_time, None);
        assert_eq!(blocks[0].speed, None);
    }

    #[test]
    fn one_hundred_percent_arrives_once_and_only_from_finish() {
        let start = Instant::now();
        let mut reporter = ProgressReporter::new(Duration::from_secs(10));
        let past_the_end = ProgressBlock {
            out_time: Some(Duration::from_secs(11)),
            speed: None,
            end: true,
        };
        let update = reporter.block(&past_the_end, start).expect("first update");
        assert!(update.fraction < 1.0);

        assert_eq!(reporter.finish().map(|p| p.fraction), Some(1.0));
        assert_eq!(reporter.finish(), None);
        assert_eq!(
            reporter.block(&past_the_end, start + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn updates_are_throttled_and_never_go_backwards() {
        let start = Instant::now();
        let mut reporter = ProgressReporter::new(Duration::from_secs(100));
        let at = |secs| ProgressBlock {
            out_time: Some(Duration::from_secs(secs)),
            speed: None,
            end: false,
        };
        assert!(reporter.block(&at(10), start).is_some());
        assert!(
            reporter
                .block(&at(20), start + Duration::from_millis(10))
                .is_none()
        );
        assert!(
            reporter
                .block(&at(5), start + Duration::from_millis(500))
                .is_none()
        );
        let later = reporter
            .block(&at(30), start + Duration::from_millis(600))
            .expect("due");
        assert!((later.fraction - 0.3).abs() < 1e-9);
    }
}
