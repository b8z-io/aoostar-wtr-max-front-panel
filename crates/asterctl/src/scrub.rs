// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025 Markus Zehnder

//! Screen conditioning to counter LCD image retention.
//!
//! This panel ships displaying a single static vendor logo and leaves it up indefinitely
//! when nothing is driving it. Months of that burns a persistent ghost into the liquid
//! crystal, which is plainly visible behind a dark background.
//!
//! A sensor panel has the same problem in slower motion: fixed tiles, captions that never
//! move, and digits that sit in the same place for years. Retention is not a defect to fix
//! once — it is a continuous cost of showing a static layout, and it needs a continuous
//! remedy.
//!
//! The remedy is to periodically drive every pixel through its full range. Full-field
//! flashes swing all subpixels between extremes, the primaries exercise each colour channel
//! independently (retention can affect one channel more than the others), and noise frames
//! break up any spatial pattern the display has settled into.
//!
//! # Cost
//!
//! Every scrub frame differs from its predecessor in essentially every pixel, so the frame
//! cache cannot help and each one costs a full transfer — roughly 1.3s over this panel's
//! 12 Mbps link. That is the price of the technique, not an inefficiency to optimise away:
//! a scrub frame that changed only part of the screen would only condition part of it.
//!
//! Scrubbing is therefore something to do occasionally and briefly. Once an hour for twenty
//! seconds costs well under 1% of the link.

use image::{Rgb, RgbImage};
use log::{debug, warn};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default time a single scrub runs for.
pub const DEFAULT_SCRUB_DURATION: Duration = Duration::from_secs(20);

/// One step of the conditioning cycle.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ScrubFrame {
    /// Full-field solid colour. White and black swing every subpixel between extremes;
    /// the primaries exercise one channel at a time.
    Solid(Rgb<u8>),
    /// Per-pixel random values, to break up any spatial pattern.
    Noise,
}

/// The conditioning cycle, in order.
///
/// Full-range swings first, since they do most of the work, then the primaries, then noise.
/// The cycle repeats for as long as the scrub is configured to run.
const CYCLE: [ScrubFrame; 10] = [
    ScrubFrame::Solid(Rgb([255, 255, 255])),
    ScrubFrame::Solid(Rgb([0, 0, 0])),
    ScrubFrame::Solid(Rgb([255, 255, 255])),
    ScrubFrame::Solid(Rgb([0, 0, 0])),
    ScrubFrame::Solid(Rgb([255, 0, 0])),
    ScrubFrame::Solid(Rgb([0, 255, 0])),
    ScrubFrame::Solid(Rgb([0, 0, 255])),
    ScrubFrame::Noise,
    ScrubFrame::Noise,
    ScrubFrame::Noise,
];

/// Generates conditioning frames in cycle order.
///
/// Deliberately not an infinite iterator of identical noise: the caller decides when to
/// stop, based on elapsed wall-clock time rather than a frame count, because frame transfer
/// time depends on the link.
pub struct ScrubSequence {
    size: (u32, u32),
    index: usize,
    rng: XorShift32,
}

impl ScrubSequence {
    /// `seed` varies the noise between runs. Any non-zero value works; zero is replaced,
    /// since a zero state would leave the generator stuck.
    pub fn new(size: (u32, u32), seed: u32) -> Self {
        Self {
            size,
            index: 0,
            rng: XorShift32::new(seed),
        }
    }

    /// The kind of frame that will be produced next, without generating it.
    pub fn peek_kind(&self) -> ScrubFrame {
        CYCLE[self.index % CYCLE.len()]
    }

    /// True when the next frame starts a fresh cycle.
    ///
    /// A partial cycle conditions the display unevenly — stopping after white and black
    /// exercises the extremes but never the individual colour channels or the noise pass —
    /// so callers should stop only on a boundary.
    pub fn at_cycle_start(&self) -> bool {
        self.index.is_multiple_of(CYCLE.len())
    }

    /// Number of frames in one complete cycle.
    pub fn cycle_len() -> usize {
        CYCLE.len()
    }
}

impl Iterator for ScrubSequence {
    type Item = RgbImage;

    fn next(&mut self) -> Option<RgbImage> {
        let kind = self.peek_kind();
        self.index = self.index.wrapping_add(1);

        let (w, h) = self.size;
        let img = match kind {
            ScrubFrame::Solid(colour) => RgbImage::from_pixel(w, h, colour),
            ScrubFrame::Noise => {
                let mut img = RgbImage::new(w, h);
                for pixel in img.pixels_mut() {
                    let v = self.rng.next();
                    *pixel = Rgb([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
                }
                img
            }
        };

        Some(img)
    }
}

/// How long ago the last scrub ran, according to the state file.
///
/// The in-process timer resets every time the service restarts, so a run of deploys or a
/// crash loop can starve the display of conditioning indefinitely — three restarts during
/// one deployment pushed the next scrub out by three full intervals.
///
/// Wall-clock time is used here, unlike the monotonic [`Instant`](std::time::Instant) the
/// running timer uses, because it has to survive a process boundary. A clock step means at
/// worst one extra or one skipped scrub, which does not matter; the state file lives in
/// tmpfs and vanishes on reboot, which is the right scope.
///
/// Returns `None` when there is no usable state, in which case the caller should start its
/// interval fresh.
pub fn read_state(path: &Path) -> Option<Duration> {
    let contents = fs::read_to_string(path).ok()?;
    let seconds: u64 = contents.trim().parse().ok()?;
    let written = UNIX_EPOCH.checked_add(Duration::from_secs(seconds))?;

    match SystemTime::now().duration_since(written) {
        Ok(elapsed) => Some(elapsed),
        // The file is stamped in the future: the clock moved backwards. Treat it as
        // just-scrubbed rather than scrubbing immediately.
        Err(_) => Some(Duration::ZERO),
    }
}

/// Record that a scrub has just run.
///
/// Best-effort: a failure here costs a mistimed scrub, never a failed render, so it warns
/// rather than propagating.
pub fn write_state(path: &Path) {
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        warn!("System clock is before the epoch; not recording scrub time");
        return;
    };
    match fs::write(path, now.as_secs().to_string()) {
        Ok(()) => debug!("Recorded scrub time in {path:?}"),
        Err(e) => warn!("Could not record scrub time in {path:?}: {e}"),
    }
}

/// Successive small offsets for the whole rendered panel.
///
/// The scrub treats retention that has already happened. This prevents our own layout from
/// creating more of it: a tile edge or a digit that sits on exactly the same pixels for
/// years will eventually etch itself in, however good the conditioning cycle is. Moving the
/// image by a pixel or two spreads that wear over a small neighbourhood instead.
///
/// The offsets walk a ring rather than jumping randomly, so successive positions are
/// adjacent and the movement is imperceptible in normal viewing.
///
/// # Cost
///
/// Shifting changes every chunk, so it costs a full frame. Advancing the offset at the same
/// moment as a scrub makes it free: the post-scrub frame is a full redraw regardless.
pub struct PixelShift {
    max: i32,
    index: usize,
}

/// Unit ring, scaled by the configured maximum.
const RING: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

impl PixelShift {
    pub fn new(max: i32) -> Self {
        Self {
            max: max.max(0),
            index: 0,
        }
    }

    /// Advance to the next position and return it.
    pub fn advance(&mut self) -> (i32, i32) {
        if self.max == 0 {
            return (0, 0);
        }
        let (dx, dy) = RING[self.index % RING.len()];
        self.index = self.index.wrapping_add(1);
        (dx * self.max, dy * self.max)
    }

    /// Number of distinct positions before the pattern repeats.
    pub fn positions() -> usize {
        RING.len()
    }
}

/// Small deterministic PRNG.
///
/// Noise here only needs to be visually unstructured, not statistically strong, and adding a
/// dependency for it would work against keeping this change upstreamable.
struct XorShift32(u32);

impl XorShift32 {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (64, 32);

    #[test]
    fn cycle_starts_with_a_full_range_swing() {
        let mut seq = ScrubSequence::new(SIZE, 1);
        let white = seq.next().expect("infinite sequence");
        let black = seq.next().expect("infinite sequence");

        assert_eq!(*white.get_pixel(0, 0), Rgb([255, 255, 255]));
        assert_eq!(*black.get_pixel(0, 0), Rgb([0, 0, 0]));
    }

    #[test]
    fn every_frame_is_the_display_size() {
        let seq = ScrubSequence::new(SIZE, 7);
        for img in seq.take(CYCLE.len() * 2) {
            assert_eq!(img.dimensions(), SIZE);
        }
    }

    #[test]
    fn primaries_exercise_one_channel_each() {
        let mut seq = ScrubSequence::new(SIZE, 1);
        let frames: Vec<_> = (&mut seq).take(7).collect();
        assert_eq!(*frames[4].get_pixel(3, 3), Rgb([255, 0, 0]));
        assert_eq!(*frames[5].get_pixel(3, 3), Rgb([0, 255, 0]));
        assert_eq!(*frames[6].get_pixel(3, 3), Rgb([0, 0, 255]));
    }

    #[test]
    fn noise_frames_are_not_uniform() {
        let mut seq = ScrubSequence::new(SIZE, 42);
        let noise = seq.nth(7).expect("eighth frame is noise");

        let first = *noise.get_pixel(0, 0);
        assert!(
            noise.pixels().any(|p| *p != first),
            "a noise frame that is uniform would condition nothing"
        );
    }

    #[test]
    fn consecutive_noise_frames_differ() {
        let mut seq = ScrubSequence::new(SIZE, 42);
        let a = seq.nth(7).expect("eighth frame is noise");
        let b = seq.next().expect("ninth frame is noise");

        assert_ne!(
            a.as_raw(),
            b.as_raw(),
            "repeating the same noise would leave a fixed pattern on screen"
        );
    }

    #[test]
    fn cycle_boundaries_are_reported() {
        let mut seq = ScrubSequence::new(SIZE, 3);
        assert!(seq.at_cycle_start(), "a fresh sequence starts a cycle");
        seq.next();
        assert!(!seq.at_cycle_start(), "mid-cycle after one frame");
        for _ in 1..ScrubSequence::cycle_len() {
            seq.next();
        }
        assert!(seq.at_cycle_start(), "boundary again after a full cycle");
    }

    #[test]
    fn the_cycle_repeats() {
        let mut seq = ScrubSequence::new(SIZE, 3);
        for _ in 0..CYCLE.len() {
            seq.next();
        }
        assert_eq!(seq.peek_kind(), CYCLE[0], "cycle should wrap around");
    }

    #[test]
    fn state_round_trips_as_almost_no_elapsed_time() {
        let dir = std::env::temp_dir().join("asterctl-scrub-state-roundtrip");
        let _ = fs::remove_file(&dir);
        write_state(&dir);

        let elapsed = read_state(&dir).expect("state just written should read back");
        assert!(elapsed < Duration::from_secs(5), "got {elapsed:?}");
        let _ = fs::remove_file(&dir);
    }

    #[test]
    fn a_missing_state_file_reads_as_none() {
        let path = std::env::temp_dir().join("asterctl-scrub-state-does-not-exist");
        let _ = fs::remove_file(&path);
        assert_eq!(read_state(&path), None);
    }

    #[test]
    fn an_old_timestamp_reports_a_long_elapsed_time() {
        let path = std::env::temp_dir().join("asterctl-scrub-state-old");
        let long_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7200;
        fs::write(&path, long_ago.to_string()).unwrap();

        let elapsed = read_state(&path).expect("readable");
        assert!(elapsed >= Duration::from_secs(7100), "got {elapsed:?}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_future_timestamp_reads_as_just_scrubbed() {
        let path = std::env::temp_dir().join("asterctl-scrub-state-future");
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 9000;
        fs::write(&path, future.to_string()).unwrap();

        assert_eq!(
            read_state(&path),
            Some(Duration::ZERO),
            "a backwards clock must not trigger an immediate scrub"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rubbish_in_the_state_file_reads_as_none() {
        let path = std::env::temp_dir().join("asterctl-scrub-state-rubbish");
        fs::write(&path, "not a timestamp").unwrap();
        assert_eq!(read_state(&path), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn pixel_shift_stays_within_bounds() {
        let mut shift = PixelShift::new(2);
        for _ in 0..PixelShift::positions() * 3 {
            let (dx, dy) = shift.advance();
            assert!(
                dx.abs() <= 2 && dy.abs() <= 2,
                "offset {dx},{dy} out of bounds"
            );
        }
    }

    #[test]
    fn pixel_shift_visits_every_position_before_repeating() {
        let mut shift = PixelShift::new(1);
        let seen: std::collections::HashSet<_> = (0..PixelShift::positions())
            .map(|_| shift.advance())
            .collect();
        assert_eq!(seen.len(), PixelShift::positions());
    }

    #[test]
    fn pixel_shift_moves_to_an_adjacent_position() {
        let mut shift = PixelShift::new(1);
        let mut prev = shift.advance();
        for _ in 0..PixelShift::positions() {
            let next = shift.advance();
            let step = (next.0 - prev.0).abs().max((next.1 - prev.1).abs());
            assert!(step <= 1, "jumped from {prev:?} to {next:?}");
            prev = next;
        }
    }

    #[test]
    fn a_zero_maximum_disables_the_shift() {
        let mut shift = PixelShift::new(0);
        assert_eq!(shift.advance(), (0, 0));
        assert_eq!(shift.advance(), (0, 0));
    }

    #[test]
    fn a_zero_seed_still_produces_noise() {
        let mut seq = ScrubSequence::new(SIZE, 0);
        let noise = seq.nth(7).expect("eighth frame is noise");

        let first = *noise.get_pixel(0, 0);
        assert!(noise.pixels().any(|p| *p != first));
    }

    #[test]
    fn a_given_seed_is_reproducible() {
        let a: Vec<_> = ScrubSequence::new(SIZE, 99).take(9).collect();
        let b: Vec<_> = ScrubSequence::new(SIZE, 99).take(9).collect();
        assert_eq!(a.last().unwrap().as_raw(), b.last().unwrap().as_raw());
    }
}
