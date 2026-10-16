// Independent median-split XY index: artifact comparison never calls CAM offsets.
pub(crate) struct Index {
    min: [f64; 2],
    max: [f64; 2],
    items: Vec<(usize, [f64; 2], [f64; 2])>,
    children: Vec<Index>,
}
impl Index {
    pub(crate) fn new(mut items: Vec<(usize, [f64; 2], [f64; 2])>) -> Self {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for (_, a, b) in &items {
            for k in 0..2 {
                min[k] = min[k].min(a[k]);
                max[k] = max[k].max(b[k]);
            }
        }
        if items.len() <= 8 {
            return Self { min, max, items, children: vec![] };
        }
        let axis = usize::from(max[1] - min[1] > max[0] - min[0]);
        items.sort_unstable_by(|a, b| (a.1[axis] + a.2[axis]).total_cmp(&(b.1[axis] + b.2[axis])));
        let rest = items.split_off(items.len() / 2);
        Self { min, max, items: vec![], children: vec![Self::new(items), Self::new(rest)] }
    }
    pub(crate) fn query(&self, min: [f64; 2], max: [f64; 2], out: &mut Vec<usize>) {
        if (0..2).any(|k| self.min[k] > max[k] || self.max[k] < min[k]) {
            return;
        }
        out.extend(self.items.iter().filter(|(_, a, b)| (0..2).all(|k| a[k] <= max[k] && b[k] >= min[k])).map(|(id, _, _)| id));
        for child in &self.children {
            child.query(min, max, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_candidates_include_every_brute_force_overlap() {
        let items: Vec<_> = (0..1024)
            .map(|i| {
                let x = (i % 32) as f64 - 16.;
                let y = (i / 32) as f64 - 16.;
                (i, [x, y], [x + 0.5, y + 0.75])
            })
            .collect();
        let index = Index::new(items.clone());
        for (min, max) in [([-0.5, -2.], [3.5, 4.]), ([-16., -16.], [-16., -16.]), ([16., 16.], [17., 17.]), ([-4.75, -7.5], [-4.5, -7.25])] {
            let mut actual = Vec::new();
            index.query(min, max, &mut actual);
            for (id, a, b) in &items {
                if (0..2).all(|k| a[k] <= max[k] && b[k] >= min[k]) {
                    assert!(actual.contains(id));
                }
            }
            let expected: Vec<_> = items.iter().filter(|(_, a, b)| (0..2).all(|k| a[k] <= max[k] && b[k] >= min[k])).map(|(id, _, _)| *id).collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
            let unique: std::collections::BTreeSet<_> = actual.iter().collect();
            assert_eq!(unique.len(), actual.len());
        }
    }
}
