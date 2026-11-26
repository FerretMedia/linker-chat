use std::ops::Range;

/// Replace `range` of the old list with `count` new items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub count: usize,
}

/// Splices that turn `old` into `new`, to apply in order.
///
/// Rows are strictly increasing by position but not by value, and a live feed
/// mostly drops from the front (eviction) and grows at the back, so those two
/// ends are reported separately. That keeps a list's measured heights for the
/// rows in between.
pub fn diff_rows(old: &[u64], new: &[u64]) -> Vec<Splice> {
    let evicted = match new.first() {
        Some(first) => old.iter().position(|row| row == first).unwrap_or(0),
        None => 0,
    };
    let kept = &old[evicted..];
    let prefix = kept.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = kept[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let middle = Splice {
        range: prefix..kept.len() - suffix,
        count: new.len() - suffix - prefix,
    };
    let mut splices = Vec::new();
    if evicted > 0 {
        splices.push(Splice {
            range: 0..evicted,
            count: 0,
        });
    }
    if !middle.range.is_empty() || middle.count > 0 {
        splices.push(middle);
    }
    splices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies splices to row values the way a list widget would, with new rows
    /// taken from their final position.
    fn apply(old: &[u64], new: &[u64]) -> Vec<u64> {
        let mut rows = old.to_vec();
        for splice in diff_rows(old, new) {
            let start = splice.range.start;
            let fresh = new[start..start + splice.count].to_vec();
            rows.splice(splice.range, fresh);
        }
        rows
    }

    #[test]
    fn rebuilds_every_kind_of_change() {
        let cases: [(&[u64], &[u64]); 9] = [
            (&[], &[1, 2, 3]),
            (&[1, 2, 3], &[]),
            (&[1, 2], &[1, 2, 3]),
            (&[1, 2, 3], &[2, 3, 4]),
            (&[1, 2, 4], &[1, 2, 3, 4]),
            (&[1, 2, 3, 4], &[1, 4]),
            (&[1, 2, 3], &[7, 8]),
            (&[1, 2, 3, 4, 5], &[3, 9, 5, 6]),
            (&[1, 1, 1], &[1, 1]),
        ];
        for (old, new) in cases {
            assert_eq!(apply(old, new), new, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn unchanged_rows_need_no_splice() {
        assert_eq!(diff_rows(&[1, 2], &[1, 2]), []);
    }

    #[test]
    fn eviction_with_append_leaves_the_middle_alone() {
        assert_eq!(
            diff_rows(&[1, 2, 3], &[2, 3, 4]),
            [
                Splice {
                    range: 0..1,
                    count: 0
                },
                Splice {
                    range: 2..2,
                    count: 1
                },
            ]
        );
    }
}
