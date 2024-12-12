use rand::prelude::SliceRandom;
use rand::Rng;
use rand::{RngCore, SeedableRng};
use rand_pcg::Mcg128Xsl64;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Mutex;

struct RandWrap {
    r: Mcg128Xsl64,
    s: u64,
}

impl RandWrap {
    fn new(seed: u64) -> Self {
        Self {
            r: Mcg128Xsl64::seed_from_u64(seed),
            s: seed,
        }
    }

    fn set_state_to(&mut self, offset: u64) {
        self.r = Mcg128Xsl64::seed_from_u64(self.s);
        self.r.advance(offset as u128);
    }

    fn next_u64(&mut self) -> u64 {
        self.r.next_u64()
    }
}

// Expectation is we set the offset and walk the number of blocks 8 bytes at a time
pub trait PatternGenerator {
    fn setup(&mut self, offset: u64);
    fn next_u64(&mut self) -> u64;
    fn seed(&self) -> u64;
}

// Eventually we want an interface and different data types that implement that interface that
// each have unique behavior we are looking for.
pub struct TestBdState {
    pub s: Rc<Mutex<Box<dyn PatternGenerator>>>,
}

impl std::clone::Clone for TestBdState {
    fn clone(&self) -> Self {
        Self { s: self.s.clone() }
    }
}

pub struct PercentPattern {
    pub fill: u32,
    pub duplicates: u32,
    pub random: u32,
}

pub struct DataMix {
    mapping: Vec<(std::ops::Range<u64>, Bucket)>,
    seed: u64,
    r: RandWrap,
    offset: u64,
    range_end: u64,
    cur_bucket: Bucket, // When we switch buckets we will need to reset the random offset
    dupe_block: [u64; 512],
}

impl DataMix {
    pub fn create(
        size: u64,
        seed: u64,
        segments: usize,
        percents: &PercentPattern,
    ) -> Box<dyn PatternGenerator> {
        let seed = if seed == 0 {
            let mut rng = rand::thread_rng();
            rng.gen_range(1..u64::MAX)
        } else {
            seed
        };

        let mapping = split_range_into_random_subranges_with_buckets(
            seed,
            size,
            segments,
            (percents.fill, percents.duplicates, percents.random),
        );

        let mut dupe_block: [u64; 512] = [0; 512];
        for (idx, d) in dupe_block.iter_mut().enumerate() {
            *d = (idx % 256) as u64;
        }

        Box::new(Self {
            mapping,
            seed,
            r: RandWrap::new(seed),
            offset: 0,
            range_end: 0,
            cur_bucket: Bucket::Random,
            dupe_block,
        })
    }

    fn find_segment(&self, offset: u64) -> Option<(std::ops::Range<u64>, Bucket)> {
        let index = match self
            .mapping
            .binary_search_by_key(&offset, |(start, _)| start.start)
        {
            Ok(i) => i,
            Err(i) => i,
        };

        for i in [index, index.wrapping_sub(1)] {
            if let Some((r, bucket)) = self.mapping.get(i) {
                if r.start <= offset && offset < r.end {
                    return Some((r.clone(), bucket.clone()));
                }
            }
        }
        None
    }
}

impl PatternGenerator for DataMix {
    fn setup(&mut self, offset: u64) {
        // If we are processing sequentially, we could already be in the correct spot
        if self.offset != offset {
            self.offset = offset;
            self.r.set_state_to(offset);

            let (range, bucket) = self.find_segment(self.offset).unwrap();
            assert!(range.start <= offset && offset < range.end);
            self.cur_bucket = bucket;
            self.range_end = range.end;
        }
    }

    fn next_u64(&mut self) -> u64 {
        // Check to see if we passed a segment boundary
        if self.offset >= self.range_end {
            self.setup(self.offset);
        }

        let v = match self.cur_bucket {
            Bucket::Fill => 0, //TODO: Make configurable?
            Bucket::Random => self.r.next_u64(),
            Bucket::Duplicate => self.dupe_block[(self.offset % 512) as usize],
        };

        self.offset += 1;
        v
    }
    fn seed(&self) -> u64 {
        self.seed
    }
}

#[derive(Debug, Clone)]
enum Bucket {
    Fill,
    Duplicate,
    Random,
}

fn split_range_into_random_subranges_with_buckets(
    seed: u64,
    range_size: u64,
    num_buckets: usize,
    percentages: (u32, u32, u32),
) -> Vec<(std::ops::Range<u64>, Bucket)> {
    assert!(
        num_buckets as u64 <= range_size,
        "Cannot split into more subranges than elements in the range"
    );
    assert!(
        percentages.0 + percentages.1 + percentages.2 == 100,
        "Percentages must sum to 100"
    );

    let mut rng = Mcg128Xsl64::seed_from_u64(seed);
    let mut split_points = BTreeSet::new();

    // Generate n-1 unique split points
    while split_points.len() < num_buckets - 1 {
        let point = rng.gen_range(512..range_size);
        split_points.insert(point);
    }

    // Collect, the points will be sorted from the BTreeSet
    let split_points: Vec<u64> = split_points.into_iter().collect();

    // Create subranges from the sorted split points
    let mut subranges = Vec::new();
    let mut start = 0;

    for &end in &split_points {
        subranges.push(start..end);
        start = end;
    }

    // Add the last range
    subranges.push(start..range_size);

    // Shuffle and distribute subranges into buckets based on percentages
    subranges.shuffle(&mut rng);

    let bucket_sizes = (
        num_buckets * percentages.0 as usize / 100,
        num_buckets * percentages.1 as usize / 100,
        num_buckets * percentages.2 as usize / 100,
    );

    let mut result = Vec::new();

    for (i, range) in subranges.into_iter().enumerate() {
        if i < bucket_sizes.0 {
            result.push((range, Bucket::Fill));
        } else if i < bucket_sizes.0 + bucket_sizes.1 {
            result.push((range, Bucket::Duplicate));
        } else {
            result.push((range, Bucket::Random));
        }
    }

    // Sort ranges by their start values to maintain order
    result.sort_by_key(|(range, _)| range.start);

    for r in &result {
        println!("{:?} - {:?}", r.0, r.1);
    }

    result
}
