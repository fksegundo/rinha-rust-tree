use crate::QueryVector;
use crate::index::build::Reference;

#[derive(Clone, Debug)]
pub struct PartitionScheme {
    pub name: String,
    pub amount_cut_count: usize,
    pub dow_cut_count: usize,
}

impl PartitionScheme {
    pub fn new(name: &str, amount_cut_count: usize, dow_cut_count: usize) -> Self {
        Self {
            name: name.to_string(),
            amount_cut_count,
            dow_cut_count,
        }
    }

    pub fn recommended() -> Self {
        Self::new("amt16_dow7", 15, 6)
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "amt32_dow7" => Some(Self::new("amt32_dow7", 31, 6)),
            "amt16_dow7" => Some(Self::new("amt16_dow7", 15, 6)),
            "amt32_only" => Some(Self::new("amt32_only", 31, 0)),
            "legacy_r2" => Some(Self::new("legacy_r2", 7, 0)),
            _ => None,
        }
    }

    pub fn compute_cuts(&self, references: &[Reference]) -> Vec<i16> {
        let mut cuts = Vec::with_capacity(self.amount_cut_count + self.dow_cut_count);

        // Compute cuts for amount (dim 0)
        let amount_cuts = compute_quantile_cuts(references, 0, self.amount_cut_count);
        cuts.extend(amount_cuts);

        // Compute cuts for day_of_week (dim 4)
        if self.dow_cut_count > 0 {
            let dow_cuts = compute_quantile_cuts(references, 4, self.dow_cut_count);
            cuts.extend(dow_cuts);
        }

        cuts
    }

    pub fn compute_key(&self, vector: &QueryVector, cuts: &[i16]) -> u32 {
        let amt_cuts = &cuts[0..self.amount_cut_count];
        let amt = bucket(vector[0], amt_cuts);
        if self.dow_cut_count == 0 {
            return amt;
        }
        let dow_cuts = &cuts[self.amount_cut_count..self.amount_cut_count + self.dow_cut_count];
        let dow = bucket(vector[4], dow_cuts);
        let dow_shift = bit_width(self.amount_cut_count + 1);
        amt | (dow << dow_shift)
    }
}

#[inline]
pub fn bucket(value: i16, cuts: &[i16]) -> u32 {
    let mut bucket = 0u32;
    for &c in cuts {
        if value > c {
            bucket += 1;
        } else {
            break;
        }
    }
    bucket
}

#[inline]
pub fn bit_width(buckets: usize) -> u32 {
    if buckets <= 1 {
        1
    } else {
        (buckets as f64).log2().ceil() as u32
    }
}

fn compute_quantile_cuts(references: &[Reference], dim: usize, cut_count: usize) -> Vec<i16> {
    if references.is_empty() || cut_count == 0 {
        return vec![0; cut_count];
    }
    let mut values: Vec<i16> = references.iter().map(|r| r.vector[dim]).collect();
    values.sort_unstable();
    let n = values.len();
    let buckets = cut_count + 1;
    let mut cuts = Vec::with_capacity(cut_count);
    for i in 0..cut_count {
        let idx = ((i + 1) * n) / buckets;
        let idx = idx.min(n - 1);
        cuts.push(values[idx]);
    }
    cuts
}
