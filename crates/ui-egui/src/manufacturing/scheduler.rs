//! Clock-independent cooperative scheduling; the web host supplies its deadline.

/// Poll a round-robin sequence until all jobs settle or the host budget expires.
/// `poll` returns true for pending work. The count cap also bounds a stalled clock.
pub(super) fn tick(cursor: &mut usize, count: usize, mut poll: impl FnMut(usize) -> bool, mut has_time: impl FnMut() -> bool) -> bool {
    if count == 0 {
        *cursor = 0;
        return false;
    }
    let mut idle = 0;
    for _ in 0..65_536 {
        if !has_time() {
            return true;
        }
        let index = *cursor % count;
        *cursor = (index + 1) % count;
        if poll(index) {
            idle = 0;
        } else {
            idle += 1;
            if idle == count {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_slices_do_not_starve_jobs_and_settled_jobs_stop_repainting() {
        let mut cursor = 0;
        let mut remaining = [7_usize, 1, 5];
        let mut order = Vec::new();
        for _ in 0..30 {
            let mut budget = 1;
            let pending = tick(
                &mut cursor,
                remaining.len(),
                |i| {
                    order.push(i);
                    remaining[i] = remaining[i].saturating_sub(1usize);
                    remaining[i] > 0
                },
                || {
                    let more = budget > 0;
                    budget -= usize::from(more);
                    more
                },
            );
            assert!(pending);
        }
        assert!(order.as_chunks::<3>().0.iter().all(|chunk| *chunk == [0, 1, 2]));
        assert_eq!(remaining, [0; 3]);
        assert!(!tick(&mut cursor, 3, |_| false, || true));
        // Removing completed jobs must tolerate a cursor past the new length.
        cursor = 12;
        assert!(!tick(
            &mut cursor,
            1,
            |i| {
                assert_eq!(i, 0);
                false
            },
            || true
        ));
        assert!(!tick(&mut cursor, 0, |_| true, || true));
    }
    #[test]
    fn clock_stall_and_empty_budget_are_bounded() {
        let mut cursor = 0;
        let mut polls = 0;
        assert!(tick(
            &mut cursor,
            2,
            |_| {
                polls += 1;
                true
            },
            || false
        ));
        assert_eq!(polls, 0);
        assert!(tick(
            &mut cursor,
            2,
            |_| {
                polls += 1;
                true
            },
            || true
        ));
        assert_eq!(polls, 65_536);
    }
}
