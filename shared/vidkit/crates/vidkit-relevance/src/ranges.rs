//! Fetching only the sections worth watching.
//!
//! Seeking past a section still downloads it if the byte source is reading
//! ahead, so a skip plan that only moves the playhead saves the viewer's time
//! and none of their data. On a metered phone connection that is the wrong half
//! of the benefit.
//!
//! This turns a plan of time ranges into a plan of BYTE ranges, using the
//! sample index the demuxer already built. A section the plan skips is then
//! never requested.
//!
//! # Why this is a separate step
//!
//! The demuxers know where every sample is; the relevance planner knows which
//! seconds matter; neither should know about the other. This is the join, and
//! it is pure — given a sample table and a set of time ranges it returns byte
//! ranges, with no IO and nothing to mock.

use std::time::Duration;

/// One sample's position in the file, which is all this needs from a demuxer.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct SampleSpan {
    pub start: Duration,
    pub offset: u64,
    pub size: u32,
}

/// A half-open byte range, `[start, end)`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
    fn touches(&self, other: &ByteRange, gap: u64) -> bool {
        // Overlapping, adjacent, or separated by less than `gap`.
        self.start.saturating_sub(gap) <= other.end && other.start.saturating_sub(gap) <= self.end
    }
}

/// A time range worth fetching.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct TimeRange {
    pub start: Duration,
    pub end: Duration,
}

#[derive(Copy, Clone, Debug)]
pub struct RangePolicy {
    /// Merge byte ranges separated by less than this.
    ///
    /// Two requests for ranges 8 KB apart cost more than one request that
    /// includes the 8 KB between them: a round trip on a phone radio is tens of
    /// milliseconds and wakes it from idle, which is its own battery cost.
    pub merge_gap: u64,
    /// Extra bytes fetched before each kept section.
    ///
    /// A decoder needs the keyframe the section starts at, and headers may sit
    /// just before the first sample. Starting exactly at the first byte of a
    /// section reliably produces a decoder that has nothing to start from.
    pub lead_in: u64,
}

impl Default for RangePolicy {
    fn default() -> Self {
        Self {
            merge_gap: 64 * 1024,
            lead_in: 8 * 1024,
        }
    }
}

/// Byte ranges covering the samples inside `keep`.
///
/// `samples` must be sorted by offset, which is how both demuxers build them.
pub fn byte_ranges(samples: &[SampleSpan], keep: &[TimeRange], policy: &RangePolicy) -> Vec<ByteRange> {
    if samples.is_empty() || keep.is_empty() {
        return Vec::new();
    }

    let mut raw: Vec<ByteRange> = Vec::new();
    for s in samples {
        // A sample belongs to a range if it STARTS inside it. A sample
        // straddling the boundary is included by the range it starts in, so it
        // is fetched exactly once rather than by both or neither.
        if keep.iter().any(|r| s.start >= r.start && s.start < r.end) {
            raw.push(ByteRange {
                start: s.offset,
                end: s.offset + s.size as u64,
            });
        }
    }
    if raw.is_empty() {
        return Vec::new();
    }

    raw.sort_by_key(|r| r.start);

    let mut out: Vec<ByteRange> = Vec::new();
    for r in raw {
        match out.last_mut() {
            Some(last) if last.touches(&r, policy.merge_gap) => {
                last.end = last.end.max(r.end);
            }
            _ => out.push(r),
        }
    }

    // Lead-in last, so it cannot create an overlap the merge already resolved.
    for r in out.iter_mut() {
        r.start = r.start.saturating_sub(policy.lead_in);
    }
    // The lead-in may have made neighbours touch; merge once more.
    let mut merged: Vec<ByteRange> = Vec::new();
    for r in out {
        match merged.last_mut() {
            Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// Bytes the plan will fetch.
pub fn planned_bytes(ranges: &[ByteRange]) -> u64 {
    ranges.iter().map(|r| r.len()).sum()
}

/// What fraction of the media the plan avoids downloading.
///
/// The number worth showing: "skips 6 minutes" is about time, this is about
/// data, and on a metered connection the second is what people care about.
pub fn saved_fraction(ranges: &[ByteRange], total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        return 0.0;
    }
    let fetched = planned_bytes(ranges).min(total_bytes);
    1.0 - (fetched as f64 / total_bytes as f64) as f32
}

/// Whether an offset is inside the plan, so a source can refuse to read
/// outside it rather than silently pulling a skipped section.
pub fn contains(ranges: &[ByteRange], offset: u64) -> bool {
    ranges.iter().any(|r| offset >= r.start && offset < r.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Ten samples, one a second, 1000 bytes each, laid out contiguously.
    fn samples() -> Vec<SampleSpan> {
        (0..10)
            .map(|i| SampleSpan {
                start: secs(i),
                offset: 10_000 + i * 1000,
                size: 1000,
            })
            .collect()
    }

    fn tight() -> RangePolicy {
        RangePolicy {
            merge_gap: 0,
            lead_in: 0,
        }
    }

    #[test]
    fn only_the_kept_seconds_are_fetched() {
        let keep = [TimeRange { start: secs(2), end: secs(5) }];
        let r = byte_ranges(&samples(), &keep, &tight());
        assert_eq!(
            r,
            vec![ByteRange { start: 12_000, end: 15_000 }],
            "expected samples 2,3,4"
        );
    }

    #[test]
    fn separate_sections_give_separate_ranges() {
        let keep = [
            TimeRange { start: secs(0), end: secs(2) },
            TimeRange { start: secs(7), end: secs(9) },
        ];
        let r = byte_ranges(&samples(), &keep, &tight());
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0], ByteRange { start: 10_000, end: 12_000 });
        assert_eq!(r[1], ByteRange { start: 17_000, end: 19_000 });
    }

    /// Two requests a few kilobytes apart cost more than one that includes the
    /// gap: a radio round trip is tens of milliseconds and wakes the modem.
    #[test]
    fn nearby_ranges_merge_into_one_request() {
        let keep = [
            TimeRange { start: secs(0), end: secs(2) },
            TimeRange { start: secs(3), end: secs(5) },
        ];
        let tight_r = byte_ranges(&samples(), &keep, &tight());
        assert_eq!(tight_r.len(), 2, "no merging at gap 0");

        let merged = byte_ranges(&samples(), &keep, &RangePolicy { merge_gap: 4096, lead_in: 0 });
        assert_eq!(merged.len(), 1, "a 1 KB gap should have merged: {merged:?}");
        assert_eq!(merged[0], ByteRange { start: 10_000, end: 15_000 });
    }

    /// A decoder starting exactly at the first byte of a section has nothing to
    /// start from; headers and the keyframe sit just before it.
    #[test]
    fn a_lead_in_is_fetched_before_each_section() {
        let keep = [TimeRange { start: secs(5), end: secs(7) }];
        let r = byte_ranges(&samples(), &keep, &RangePolicy { merge_gap: 0, lead_in: 500 });
        assert_eq!(r[0].start, 14_500, "no lead-in: {r:?}");
        assert_eq!(r[0].end, 17_000);
    }

    /// The lead-in can push a range back into its neighbour; leaving that
    /// unmerged issues two overlapping requests for the same bytes.
    #[test]
    fn a_lead_in_that_creates_an_overlap_is_merged_away() {
        let keep = [
            TimeRange { start: secs(0), end: secs(2) },
            TimeRange { start: secs(3), end: secs(5) },
        ];
        // Gap between them is 1000 bytes; a 2000-byte lead-in swallows it.
        let r = byte_ranges(&samples(), &keep, &RangePolicy { merge_gap: 0, lead_in: 2000 });
        assert_eq!(r.len(), 1, "overlapping ranges were not merged: {r:?}");
    }

    /// A sample straddling a boundary must be fetched exactly once -- not by
    /// both ranges, and not by neither.
    #[test]
    fn a_straddling_sample_belongs_to_one_range_only() {
        let s = vec![
            SampleSpan { start: secs(0), offset: 0, size: 100 },
            // Starts at 4s, runs past the 5s boundary.
            SampleSpan { start: secs(4), offset: 100, size: 100 },
            SampleSpan { start: secs(6), offset: 200, size: 100 },
        ];
        let keep = [
            TimeRange { start: secs(0), end: secs(5) },
            TimeRange { start: secs(5), end: secs(10) },
        ];
        let r = byte_ranges(&s, &keep, &tight());
        assert_eq!(planned_bytes(&r), 300, "a sample was counted twice: {r:?}");
    }

    #[test]
    fn an_empty_plan_fetches_nothing() {
        assert!(byte_ranges(&samples(), &[], &tight()).is_empty());
        assert!(byte_ranges(&[], &[TimeRange { start: secs(0), end: secs(1) }], &tight()).is_empty());
    }

    /// A plan whose ranges match no samples must fetch nothing rather than
    /// everything.
    #[test]
    fn a_plan_past_the_end_fetches_nothing() {
        let keep = [TimeRange { start: secs(100), end: secs(200) }];
        assert!(byte_ranges(&samples(), &keep, &tight()).is_empty());
    }

    #[test]
    fn saved_fraction_reports_the_data_not_the_time() {
        let keep = [TimeRange { start: secs(0), end: secs(2) }];
        let r = byte_ranges(&samples(), &keep, &tight());
        assert_eq!(planned_bytes(&r), 2000);
        // 2000 of a 10000-byte payload.
        assert!((saved_fraction(&r, 10_000) - 0.8).abs() < 0.001);
        assert_eq!(saved_fraction(&r, 0), 0.0, "no division by zero");
    }

    #[test]
    fn membership_answers_whether_an_offset_is_in_the_plan() {
        let keep = [TimeRange { start: secs(2), end: secs(4) }];
        let r = byte_ranges(&samples(), &keep, &tight());
        assert!(contains(&r, 12_000));
        assert!(contains(&r, 13_999));
        assert!(!contains(&r, 14_000), "end is exclusive");
        assert!(!contains(&r, 11_999));
    }

    /// Samples out of offset order must still produce sane ranges: a plan built
    /// from them would otherwise merge nothing and issue a request per sample.
    #[test]
    fn out_of_order_samples_are_sorted_before_merging() {
        let mut s = samples();
        s.reverse();
        let keep = [TimeRange { start: secs(0), end: secs(10) }];
        let r = byte_ranges(&s, &keep, &tight());
        assert_eq!(r.len(), 1, "reversed input produced {} ranges", r.len());
        assert_eq!(r[0], ByteRange { start: 10_000, end: 20_000 });
    }
}
