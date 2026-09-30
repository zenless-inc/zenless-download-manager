//! IDM-style dynamic segmentation.
//!
//! A download starts with `N` equal segments, one per connection. Whenever a
//! connection finishes its segment it asks for more work: first any
//! unassigned unfinished segment, otherwise the *largest remaining* active
//! segment is split in half (if more than [`MIN_SPLIT`] remains) and the
//! connection takes the second half. The first half's connection simply
//! stops earlier because its `end` moved.

use super::model::Segment;

/// Don't split a segment unless more than this remains (1 MiB).
pub const MIN_SPLIT: u64 = 1024 * 1024;
/// Smallest initial segment; tiny files get fewer connections.
pub const MIN_INITIAL_SEGMENT: u64 = 256 * 1024;
/// Upper bound for one positional write; guarantees a concurrent split
/// (which always leaves ≥ `MIN_SPLIT / 2` to the old owner) never overlaps
/// an in-flight write.
pub const MAX_WRITE: usize = 256 * 1024;

/// A segment plus whether a connection currently owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub seg: Segment,
    pub active: bool,
}

/// Splits `[0, total)` into up to `connections` equal segments, each at
/// least [`MIN_INITIAL_SEGMENT`] long (except when the file is smaller).
pub fn initial_segments(total: u64, connections: u32) -> Vec<Segment> {
    if total == 0 {
        return vec![Segment::new(0, 0)];
    }
    let max_by_size = (total / MIN_INITIAL_SEGMENT).max(1);
    let n = u64::from(connections.max(1)).min(max_by_size);
    let base = total / n;
    let mut out = Vec::with_capacity(n as usize);
    let mut start = 0;
    for i in 0..n {
        let end = if i + 1 == n { total } else { start + base };
        out.push(Segment::new(start, end));
        start = end;
    }
    out
}

/// Finds work for a connection: an idle unfinished segment, or else splits
/// the largest active one. Marks the returned slot active.
pub fn claim(slots: &mut Vec<Slot>) -> Option<usize> {
    if let Some(i) = slots.iter().position(|s| !s.active && !s.seg.is_done()) {
        slots[i].active = true;
        return Some(i);
    }
    split_largest(slots, MIN_SPLIT)
}

/// Splits the active segment with the most remaining bytes in half when
/// more than `min_split` remains. The new second half is pushed (active)
/// and its index returned.
pub fn split_largest(slots: &mut Vec<Slot>, min_split: u64) -> Option<usize> {
    let (idx, remaining) = slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.active)
        .map(|(i, s)| (i, s.seg.remaining()))
        .max_by_key(|&(_, r)| r)?;
    if remaining <= min_split {
        return None;
    }
    let old = &mut slots[idx];
    let mid = old.seg.pos + remaining / 2;
    let new = Segment::new(mid, old.seg.end);
    old.seg.end = mid;
    slots.push(Slot { seg: new, active: true });
    Some(slots.len() - 1)
}

/// Total bytes downloaded across segments.
pub fn downloaded(segments: &[Segment]) -> u64 {
    segments.iter().map(Segment::downloaded).sum()
}

/// Every byte is present.
pub fn all_done(segments: &[Segment]) -> bool {
    segments.iter().all(Segment::is_done)
}

/// Sorts by start and merges finished neighbours to keep the persisted list
/// short (unfinished segments are kept as-is).
pub fn compact(segments: &[Segment]) -> Vec<Segment> {
    let mut v: Vec<Segment> = segments.to_vec();
    v.sort_by_key(|s| s.start);
    let mut out: Vec<Segment> = Vec::with_capacity(v.len());
    for s in v {
        if let Some(last) = out.last_mut()
            && last.is_done()
            && s.is_done()
            && last.end == s.start
        {
            last.end = s.end;
            last.pos = s.end;
            continue;
        }
        out.push(s);
    }
    out
}

/// Sanity check for persisted segments: in bounds, non-overlapping, covering
/// the whole file.
pub fn validate(segments: &[Segment], total: u64) -> bool {
    let mut v: Vec<Segment> = segments.to_vec();
    v.sort_by_key(|s| s.start);
    let mut expected = 0;
    for s in &v {
        if s.start != expected || s.end < s.start || s.pos < s.start || s.pos > s.end {
            return false;
        }
        expected = s.end;
    }
    expected == total
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn slots(segs: Vec<Segment>) -> Vec<Slot> {
        segs.into_iter().map(|seg| Slot { seg, active: false }).collect()
    }

    #[test]
    fn initial_split_is_even_and_complete() {
        let segs = initial_segments(100 * MIB + 7, 8);
        assert_eq!(segs.len(), 8);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end, 100 * MIB + 7);
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert!(validate(&segs, 100 * MIB + 7));
    }

    #[test]
    fn small_files_get_fewer_segments() {
        assert_eq!(initial_segments(1000, 8).len(), 1);
        assert_eq!(initial_segments(600 * 1024, 8).len(), 2);
        assert_eq!(initial_segments(0, 8), vec![Segment::new(0, 0)]);
        assert_eq!(initial_segments(10 * MIB, 0).len(), 1);
    }

    #[test]
    fn claim_prefers_idle_segments() {
        let mut s = slots(initial_segments(8 * MIB, 4));
        assert_eq!(claim(&mut s), Some(0));
        assert_eq!(claim(&mut s), Some(1));
        assert!(s[0].active && s[1].active && !s[2].active);
    }

    #[test]
    fn splits_largest_remaining_in_half() {
        let mut s = slots(vec![Segment::new(0, 10 * MIB), Segment::new(10 * MIB, 14 * MIB)]);
        s[0].active = true;
        s[1].active = true;
        s[0].seg.pos = 6 * MIB; // 4 MiB left
        s[1].seg.pos = 10 * MIB + 512 * 1024; // 3.5 MiB left
        let idx = split_largest(&mut s, MIN_SPLIT).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(s[0].seg.end, 8 * MIB);
        assert_eq!(s[2].seg, Segment::new(8 * MIB, 10 * MIB));
        assert!(s[2].active);
        assert!(validate(&s.iter().map(|x| x.seg).collect::<Vec<_>>(), 14 * MIB));
    }

    #[test]
    fn does_not_split_small_remainders() {
        let mut s = slots(vec![Segment::new(0, MIB)]);
        s[0].active = true;
        assert_eq!(split_largest(&mut s, MIN_SPLIT), None);
        // Nothing active: nothing to split either.
        let mut s = slots(vec![Segment::new(0, 100 * MIB)]);
        s[0].seg.pos = 100 * MIB;
        assert_eq!(claim(&mut s), None);
    }

    #[test]
    fn split_leaves_room_for_in_flight_writes() {
        // The old owner always keeps at least MIN_SPLIT/2 > MAX_WRITE bytes.
        assert!(MIN_SPLIT / 2 > MAX_WRITE as u64);
    }

    #[test]
    fn simulated_download_covers_every_byte_once() {
        // Four "connections" repeatedly claim work and advance by random-ish steps.
        let total = 37 * MIB + 12345;
        let mut s = slots(initial_segments(total, 4));
        let mut owned: Vec<Option<usize>> = vec![None; 4];
        let mut written = vec![0u8; total as usize];
        let mut step = 1u64;
        loop {
            let mut progressed = false;
            for c in 0..4 {
                if owned[c].is_none() {
                    owned[c] = claim(&mut s);
                }
                let Some(i) = owned[c] else { continue };
                step = (step * 7919 + 13) % (MAX_WRITE as u64) + 1;
                let seg = &mut s[i].seg;
                let n = step.min(seg.remaining());
                for b in seg.pos..seg.pos + n {
                    written[b as usize] += 1;
                }
                seg.pos += n;
                progressed |= n > 0;
                if seg.is_done() {
                    s[i].active = false;
                    owned[c] = None;
                }
            }
            if !progressed && owned.iter().all(Option::is_none) {
                break;
            }
        }
        assert!(written.iter().all(|&w| w == 1));
        let segs: Vec<Segment> = s.iter().map(|x| x.seg).collect();
        assert!(all_done(&segs));
        assert_eq!(downloaded(&segs), total);
        assert!(s.len() > 4, "dynamic splitting should have happened");
        let c = compact(&segs);
        assert_eq!(c, vec![Segment { start: 0, end: total, pos: total }]);
    }

    #[test]
    fn validation_rejects_bad_lists() {
        assert!(!validate(&[Segment::new(0, 10)], 11));
        assert!(!validate(&[Segment::new(0, 5), Segment::new(4, 10)], 10));
        assert!(validate(&[Segment::new(5, 10), Segment::new(0, 5)], 10));
    }
}
