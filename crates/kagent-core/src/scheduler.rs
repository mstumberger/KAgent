//! Tool scheduling (port of ZCode `core/src/tool/scheduler.ts`).
//!
//! The model's call order is preserved. Consecutive concurrency-safe calls form one
//! parallel group; any call with side effects becomes its own group, so a read that
//! follows a write always observes the write.

pub fn parallel_groups(concurrency_safe: &[bool]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for (i, &safe) in concurrency_safe.iter().enumerate() {
        if safe {
            current.push(i);
        } else {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            groups.push(vec![i]);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_reads_and_isolates_writes() {
        assert_eq!(
            parallel_groups(&[true, true, false, true, false, false, true]),
            vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5], vec![6]]
        );
        assert_eq!(parallel_groups(&[true, true, true]), vec![vec![0, 1, 2]]);
        assert!(parallel_groups(&[]).is_empty());
    }
}
