// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou

use std::{cell::Cell, time::Instant};
use windows::Win32::System::{
    Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
    SystemInformation::GetTickCount64,
};

/// Selects the nearest 32-bit clock cycle, including older coalesced samples.
fn extend_milliseconds(time: u32, reference: u64) -> u64 {
    let delta = time.wrapping_sub(reference as u32) as i32;
    reference.saturating_add_signed(i64::from(delta))
}

struct PerformanceClock {
    frequency: u64,
    counter: u64,
    time_ms: u64,
}

impl PerformanceClock {
    fn milliseconds(&self, counter: u64) -> u64 {
        // Widen before multiplying. Floor historical samples too, so crossing
        // the calibration point does not change the millisecond interval.
        let elapsed = (i128::from(counter) - i128::from(self.counter)) * 1000;
        let time = i128::from(self.time_ms) + elapsed.div_euclid(i128::from(self.frequency));
        time.clamp(0, i128::from(u64::MAX)) as u64
    }
}

pub(super) struct PointerClock {
    performance: Option<PerformanceClock>,
}

impl PointerClock {
    pub(super) fn new() -> Self {
        let mut frequency = 0;
        let mut counter = 0;
        // Both queries succeed on supported Windows versions. Keep an uptime
        // fallback if the native API cannot supply a valid calibration.
        let performance = unsafe {
            if QueryPerformanceFrequency(&mut frequency).is_ok()
                && frequency > 0
                && QueryPerformanceCounter(&mut counter).is_ok()
                && counter >= 0
            {
                Some(PerformanceClock {
                    frequency: frequency as u64,
                    counter: counter as u64,
                    time_ms: GetTickCount64(),
                })
            } else {
                None
            }
        };
        Self { performance }
    }

    pub(super) fn milliseconds(&self, time: u32, performance_count: u64) -> u64 {
        self.at(time, performance_count, unsafe { GetTickCount64() })
    }

    fn at(&self, time: u32, performance_count: u64, now_ms: u64) -> u64 {
        // PerformanceCount is a high precision alternative to dwTime; in
        // particular dwTime can be zero while this counter holds the time.
        if performance_count != 0 {
            if let Some(clock) = &self.performance {
                return clock.milliseconds(performance_count);
            }
            if time == 0 {
                return now_ms;
            }
        }
        extend_milliseconds(time, now_ms)
    }
}

#[derive(Default)]
pub(super) struct WintabClock {
    last: Cell<Option<(u64, Instant)>>,
}

impl WintabClock {
    pub(super) fn milliseconds(&self, time: u32, relative: bool, received_at: Instant) -> u64 {
        let previous = self.last.get();
        let value = if relative {
            previous
                .map_or(0, |(last, _)| last)
                .saturating_add(u64::from(time))
        } else {
            // WinTab's clock origin need not match GetTickCount64. Reception
            // intervals only select the cycle after a long idle period; actual
            // sample intervals still come from the device's absolute time.
            let reference = previous.map_or(u64::from(time), |(last, received)| {
                let elapsed = received_at.saturating_duration_since(received).as_millis();
                last.saturating_add(elapsed.min(u128::from(u64::MAX)) as u64)
            });
            extend_milliseconds(time, reference)
        };
        if previous.is_none_or(|(last, _)| value >= last) {
            self.last.set(Some((value, received_at)));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const CYCLE: u64 = 1u64 << 32;

    #[test]
    fn windows_tick_history_preserves_intervals_across_rollover() {
        let clock = PointerClock { performance: None };
        let samples = [u32::MAX - 31, u32::MAX - 15, 0, 16, 32];
        for cycle in [CYCLE, 3 * CYCLE] {
            let times = samples.map(|time| clock.at(time, 0, cycle + 64));
            assert_eq!(
                times,
                [cycle - 32, cycle - 16, cycle, cycle + 16, cycle + 32]
            );
        }
    }

    #[test]
    fn windows_tick_reordering_does_not_create_a_new_cycle() {
        let clock = PointerClock { performance: None };
        assert_eq!(clock.at(101, 0, 100), 101);
        assert_eq!(clock.at(99, 0, 100), 99);
        assert_eq!(clock.at(u32::MAX - 7, 0, CYCLE + 12), CYCLE - 8);
        assert_eq!(clock.at(12, 0, CYCLE + 12), CYCLE + 12);
        assert_eq!(clock.at(12, 0, CYCLE + 12), CYCLE + 12);
    }

    #[test]
    fn windows_performance_time_keeps_zero_tick_history_and_clock_units() {
        let base_counter = 9_000_000_000_000;
        let clock = PointerClock {
            performance: Some(PerformanceClock {
                frequency: 10_000_000,
                counter: base_counter,
                time_ms: CYCLE,
            }),
        };
        for delta in [-32i64, -16, 0, 16, 32] {
            let counter = base_counter.saturating_add_signed(delta * 10_000);
            let expected = CYCLE.saturating_add_signed(delta);
            assert_eq!(clock.at(0, counter, CYCLE + 128), expected);
            assert_eq!(clock.at(expected as u32, counter, CYCLE + 128), expected);
            assert_eq!(clock.at(expected as u32, 0, CYCLE + 128), expected);
        }
    }

    #[test]
    fn performance_conversion_preserves_fractional_intervals_at_calibration() {
        let clock = PerformanceClock {
            frequency: 3_000_000,
            counter: 3_000_001,
            time_ms: 1000,
        };
        assert_eq!(clock.milliseconds(3_000_000), 999);
        assert_eq!(clock.milliseconds(3_003_000), 1000);
        let clock = PerformanceClock {
            frequency: 1,
            counter: 0,
            time_ms: 0,
        };
        assert_eq!(clock.milliseconds(u64::MAX), u64::MAX);
    }

    #[test]
    fn wintab_history_preserves_rollover_and_does_not_rebase_older_packets() {
        let clock = WintabClock::default();
        let received = Instant::now();
        let samples = [u32::MAX - 31, u32::MAX - 15, 0, 16, u32::MAX - 7, 32];
        let times = samples.map(|time| clock.milliseconds(time, false, received));
        assert_eq!(
            times,
            [
                CYCLE - 32,
                CYCLE - 16,
                CYCLE,
                CYCLE + 16,
                CYCLE - 8,
                CYCLE + 32
            ]
        );
        assert_eq!(clock.milliseconds(32, false, received), CYCLE + 32);
        assert_eq!(WintabClock::default().milliseconds(0, false, received), 0);
    }

    #[test]
    fn wintab_absolute_time_resumes_after_multiple_cycles_without_origin_assumptions() {
        let clock = WintabClock::default();
        let received = Instant::now();
        assert_eq!(clock.milliseconds(250, false, received), 250);
        let idle = 2 * CYCLE + 1234;
        let later = received + Duration::from_millis(idle);
        assert_eq!(clock.milliseconds(1484, false, later), 250 + idle);
        assert_eq!(clock.milliseconds(1500, false, later), 250 + idle + 16);
    }

    #[test]
    fn wintab_relative_time_accumulates_zero_hover_and_contact_intervals() {
        let clock = WintabClock::default();
        let received = Instant::now();
        let times =
            [0, 16, 16, 0, 32, u32::MAX].map(|time| clock.milliseconds(time, true, received));
        assert_eq!(times, [0, 16, 32, 32, 64, 64 + u64::from(u32::MAX)]);
    }

    #[test]
    fn rollover_and_performance_samples_match_the_normal_preview() {
        use efude_core::InkPoint;

        let clock = PointerClock {
            performance: Some(PerformanceClock {
                frequency: 10_000_000,
                counter: 800_000_000_000,
                time_ms: CYCLE,
            }),
        };
        let points = |times: [u64; 2]| {
            [
                InkPoint::new(4.0, 4.0, 0.4, times[0]),
                InkPoint::new(8.0, 8.0, 0.5, times[1]),
            ]
        };
        let normal = crate::estimate_preview_point(&points([1000, 1016]), 15.0, 64.0).unwrap();
        for times in [
            [
                clock.at(u32::MAX - 15, 0, CYCLE + 32),
                clock.at(0, 0, CYCLE + 32),
            ],
            [
                clock.at(0, 800_000_000_000, CYCLE + 32),
                clock.at(0, 800_000_160_000, CYCLE + 32),
            ],
        ] {
            let actual = crate::estimate_preview_point(&points(times), 15.0, 64.0).unwrap();
            assert_eq!(actual.position, normal.position);
            assert_eq!(actual.pressure, normal.pressure);
        }
    }

    #[test]
    fn windows_native_clock_calibration_uses_the_current_uptime_domain() {
        let clock = PointerClock::new();
        assert!(clock.performance.is_some());
        let before = unsafe { GetTickCount64() };
        let mut counter = 0;
        unsafe { QueryPerformanceCounter(&mut counter) }.unwrap();
        let time = clock.milliseconds(0, counter as u64);
        let after = unsafe { GetTickCount64() };
        assert!(time >= before.saturating_sub(32) && time <= after.saturating_add(32));
    }
}
