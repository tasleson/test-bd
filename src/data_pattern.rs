use rand::prelude::SliceRandom;
use rand::Rng;
use rand::{RngCore, SeedableRng};
use rand_pcg::Mcg128Xsl64;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Mutex;

use crate::position::*;

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

    fn set_state_to(&mut self, offset: IndexPos) {
        let u64_offset = offset.as_u64();
        self.r = Mcg128Xsl64::seed_from_u64(self.s);
        self.r.advance(u64_offset as u128);
    }

    fn next_u64(&mut self) -> u64 {
        self.r.next_u64()
    }
}

// Expectation is we set the offset and walk the number of blocks 8 bytes at a time
pub(crate) trait PatternGenerator {
    fn setup(&mut self, offset: IndexPos);
    fn next_u64(&mut self) -> u64;
}

// Eventually we want an interface and different data types that implement that interface that
// each have unique behavior we are looking for.
pub(crate) struct TestBdState {
    pub s: Rc<Mutex<Box<dyn PatternGenerator>>>,
}

impl std::clone::Clone for TestBdState {
    fn clone(&self) -> Self {
        Self { s: self.s.clone() }
    }
}

pub(crate) struct PercentPattern {
    pub fill: u32,
    pub duplicates: u32,
    pub random: u32,
}

pub(crate) struct DataMix {
    mapping: Vec<(std::ops::Range<IndexPos>, Bucket)>,
    r: RandWrap,
    current: IndexPos,
    start_range: IndexPos,
    end_range: IndexPos,
    cur_bucket: Bucket, // When we switch buckets we will need to reset the random offset
    dupe_block: [IndexPos; 64],
}

type PatGen = Box<dyn PatternGenerator>;
type Segments = Vec<(std::ops::Range<IndexPos>, Bucket)>;

impl DataMix {
    pub fn create(
        size: u64,
        seed: u64,
        segments: usize,
        percents: &PercentPattern,
    ) -> (PatGen, Segments) {
        assert!(size.is_multiple_of(8));

        assert!(
            percents.duplicates + percents.fill + percents.random == 100,
            "Percentages must sum to 100"
        );

        let mapping = split_range_into_random_subranges_with_buckets(
            seed,
            size,
            segments,
            (percents.fill, percents.duplicates, percents.random),
        );

        let mut dupe_block: [IndexPos; 64] = [IndexPos::new(0); 64];
        for (idx, d) in dupe_block.iter_mut().enumerate() {
            *d = IndexPos::new(idx as u64);
        }

        let mapping_copy = mapping.clone();

        (
            Box::new(Self {
                mapping,
                r: RandWrap::new(seed),
                current: IndexPos::new(u64::MAX),
                start_range: IndexPos::new(u64::MAX), // Use sentinel value to ensure first setup() always initializes
                end_range: IndexPos::new(0),
                cur_bucket: Bucket::NotValid,
                dupe_block,
            }),
            mapping_copy,
        )
    }

    fn find_segment(&self, offset: IndexPos) -> Option<(std::ops::Range<IndexPos>, Bucket)> {
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
                    return Some((r.clone(), *bucket));
                }
            }
        }
        None
    }
}

impl PatternGenerator for DataMix {
    fn setup(&mut self, offset: IndexPos) {
        // If we are processing sequentially, we could already be in the correct spot
        // Is this offset in the range we can currently handle?

        if !(self.start_range <= offset && offset < self.end_range) {
            let (range, bucket) = self.find_segment(offset).unwrap();

            assert!(range.start <= offset && offset < range.end);

            self.start_range = range.start;
            self.end_range = range.end;
            self.cur_bucket = bucket;
        }

        // If we're calling setup, we need to ensure that the random state is correct too,
        // regardless if we have changed segments
        self.r.set_state_to(offset);

        // Regardless of where we are in a range, set the current location to offset.  Then
        // as we call next we will bump this so we always know internally where we are
        self.current = offset;
    }

    fn next_u64(&mut self) -> u64 {
        // Check to see if we passed a segment boundary
        if self.current >= self.end_range {
            self.setup(self.current);
        }

        let v = match self.cur_bucket {
            Bucket::Fill => 0, //TODO: Make configurable?
            Bucket::Random => self.r.next_u64(),
            Bucket::Duplicate => self.dupe_block[(self.current.as_u64() % 64) as usize].as_u64(),
            Bucket::NotValid => panic!("Not a valid bucket"),
        };

        self.current += IndexPos::new(1);
        v
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bucket {
    Fill,

    Duplicate,

    Random,

    NotValid,
}

fn bucket_counts(
    num_buckets: usize,
    (p_fill, p_dup, p_rand): (u32, u32, u32),
) -> (usize, usize, usize) {
    assert_eq!(p_fill + p_dup + p_rand, 100);

    let weights = [
        ("fill", p_fill as f64),
        ("dup", p_dup as f64),
        ("rand", p_rand as f64),
    ];

    // Ideal fractional counts
    let ideal: Vec<(usize, f64, f64)> = weights
        .iter()
        .enumerate()
        .map(|(idx, (_, w))| {
            let v = *w * num_buckets as f64 / 100.0;
            (idx, v.floor(), v.fract())
        })
        .collect();

    let mut counts: [usize; 3] = [0; 3];
    let mut used = 0usize;

    for (idx, floor_val, _) in &ideal {
        let c = *floor_val as usize;
        counts[*idx] = c;
        used += c;
    }

    let mut remaining = num_buckets - used;

    // Give leftovers to the largest fractional parts
    let mut frac_sorted = ideal.clone();
    frac_sorted.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());

    for (idx, _, _) in frac_sorted {
        if remaining == 0 {
            break;
        }
        counts[idx] += 1;
        remaining -= 1;
    }

    (counts[0], counts[1], counts[2])
}

// Units are in bytes
fn split_range_into_random_subranges_with_buckets(
    seed: u64,
    range_size: u64,
    num_buckets: usize,
    percentages: (u32, u32, u32),
) -> Vec<(std::ops::Range<IndexPos>, Bucket)> {
    assert!(
        range_size.is_multiple_of(8),
        "total block device must be a multiple of 8"
    );

    assert!(
        num_buckets as u64 <= (range_size / 512),
        "Cannot split into more subranges than elements in the range"
    );
    assert!(
        percentages.0 + percentages.1 + percentages.2 == 100,
        "Percentages must sum to 100"
    );

    let mut rng = Mcg128Xsl64::seed_from_u64(seed);
    let mut split_points = BTreeSet::new();

    // Generate n-1 unique split points (ensure they are on 8 byte boundaries)
    let lo = 512 / 8;
    let hi = range_size / 8;
    while split_points.len() < num_buckets - 1 {
        let point = IndexPos::new(rng.gen_range(lo..hi));
        split_points.insert(point);
    }

    // Collect, the points will be sorted from the BTreeSet
    let split_points: Vec<IndexPos> = split_points.into_iter().collect();

    // Create subranges from the sorted split points
    let mut subranges: Vec<std::ops::Range<IndexPos>> = Vec::new();
    let mut start = IndexPos::new(0);

    for &end in &split_points {
        subranges.push(start..end);
        start = end;
    }

    // Add the last range
    subranges.push(start..IndexPos::new(range_size / 8));

    // Shuffle and distribute subranges into buckets based on percentages
    subranges.shuffle(&mut rng);

    let mut result = Vec::new();

    let (fill_cnt, dup_cnt, rand_cnt) = bucket_counts(num_buckets, percentages);

    for (i, range) in subranges.into_iter().enumerate() {
        if i < fill_cnt {
            result.push((range, Bucket::Fill));
        } else if i < fill_cnt + dup_cnt {
            result.push((range, Bucket::Duplicate));
        } else if i < fill_cnt + dup_cnt + rand_cnt {
            result.push((range, Bucket::Random));
        } else {
            // Should never happen if bucket_counts is correct
            unreachable!("More subranges than expected");
        }
    }

    // Sort ranges by their start values to maintain order
    result.sort_by_key(|(range, _)| range.start);

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn total_bytes(range: &Range<IndexPos>) -> u64 {
        (range.end - range.start) * 8
    }

    #[test]
    fn test_fill_pattern() {
        let (mut generator, segments) = DataMix::create(
            6656,
            12345,
            1,
            &PercentPattern {
                fill: 100,
                duplicates: 0,
                random: 0,
            },
        );

        // Verify we have one segment that's Fill type
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].1, Bucket::Fill);

        // Test at various byte offsets
        for byte_offset in (0..800).step_by(8) {
            generator.setup(IndexPos::new(byte_offset));
            let val = generator.next_u64();
            assert_eq!(
                val, 0,
                "Fill pattern should return 0 at byte offset {}",
                byte_offset
            );
        }
    }

    #[test]
    fn test_duplicate_pattern_single_read() {
        let (mut generator, segments) = DataMix::create(
            4096,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // Verify we have one segment that's Duplicate type
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].1, Bucket::Duplicate);

        // The duplicate pattern is 0, 1, 2, ..., 255, 0, 1, 2, ... (repeating)
        // When we setup() at a byte offset, the internal counter is set to that byte offset
        // Then each next_u64() call increments the counter and returns dupe_block[counter % 512]
        // dupe_block[i] = i % 256

        // Test setup at byte offset 0, then read consecutive values
        generator.setup(IndexPos::new(0));
        for i in 0..256 {
            let val = generator.next_u64();
            let expected = (i % 64) as u64;
            assert_eq!(val, expected, "Duplicate pattern at index {}", i);
        }
    }

    #[test]
    fn test_duplicate_pattern_with_offset() {
        let (mut generator, _) = DataMix::create(
            4096,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // When setup(byte_offset) is called, it sets the internal counter to byte_offset
        // Test at byte offset 16 (which means counter starts at 16)
        generator.setup(IndexPos::new(16));
        let val = generator.next_u64();
        // counter is 16, so we get dupe_block[16 % 512] = dupe_block[16] = 16 % 256 = 16
        assert_eq!(val, 16);

        // Test at byte offset 256
        generator.setup(IndexPos::new(256));
        let val = generator.next_u64();
        // counter is 256, so we get dupe_block[256 % 512] = dupe_block[256] = 256 % 256 = 0
        assert_eq!(val, 0);

        // Test at byte offset 257
        generator.setup(IndexPos::new(257));
        let val = generator.next_u64();
        // counter is 257, so we get dupe_block[257 % 512] = dupe_block[257] = 257 % 256 = 1
        assert_eq!(val, 1);
    }

    #[test]
    fn test_duplicate_pattern_repeats() {
        let (mut generator, _) = DataMix::create(
            20480,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // The pattern repeats every 64 values
        for index in 0..6 {
            // Multiple of 512 work, convert from bytes to indexes
            generator.setup(IndexPos::new(512 * index / 8));
            let v = generator.next_u64();
            assert_eq!(v, 0);
        }
    }

    #[test]
    fn test_random_pattern_deterministic() {
        let seed = 42;
        let (mut generator1, _) = DataMix::create(
            6656,
            seed,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 0,
                random: 100,
            },
        );

        let (mut generator2, _) = DataMix::create(
            6656,
            seed,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 0,
                random: 100,
            },
        );

        // Both generators with same seed should produce identical sequences
        for byte_offset in (0..800).step_by(8) {
            generator1.setup(IndexPos::new(byte_offset));
            let val1 = generator1.next_u64();

            generator2.setup(IndexPos::new(byte_offset));
            let val2 = generator2.next_u64();

            assert_eq!(
                val1, val2,
                "Random pattern should be deterministic at byte offset {}",
                byte_offset
            );
        }
    }

    #[test]
    fn test_random_pattern_different_seeds() {
        let (mut generator1, _) = DataMix::create(
            4096,
            111,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 0,
                random: 100,
            },
        );

        let (mut generator2, _) = DataMix::create(
            4096,
            222,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 0,
                random: 100,
            },
        );

        // Different seeds should produce different sequences
        let mut differences = 0;
        let mut total = 0;
        for byte_offset in (0..4096 / 8).step_by(8) {
            generator1.setup(IndexPos::new(byte_offset));
            let val1 = generator1.next_u64();

            generator2.setup(IndexPos::new(byte_offset));
            let val2 = generator2.next_u64();

            if val1 != val2 {
                differences += 1;
            }
            total += 1;
        }

        let percent_diff = differences as f64 / total as f64;

        log::debug!("percent_diff = {percent_diff}");

        assert!(
            percent_diff > 0.9,
            "Different seeds should produce mostly different values, got {} out of {total}",
            differences
        );
    }

    #[test]
    fn test_mixed_pattern_segments() {
        let (mut generator, segments) = DataMix::create(
            1024 * 10,
            12345,
            10,
            &PercentPattern {
                fill: 40,
                duplicates: 30,
                random: 30,
            },
        );

        // Verify we have 10 segments
        assert_eq!(segments.len(), 10);

        // Count the bucket types
        let fill_count = segments.iter().filter(|(_, b)| *b == Bucket::Fill).count();
        let dup_count = segments
            .iter()
            .filter(|(_, b)| *b == Bucket::Duplicate)
            .count();
        let rand_count = segments
            .iter()
            .filter(|(_, b)| *b == Bucket::Random)
            .count();

        // Verify distribution matches percentages
        assert_eq!(fill_count, 4, "Should have 4 Fill segments (40%)");
        assert_eq!(dup_count, 3, "Should have 3 Duplicate segments (30%)");
        assert_eq!(rand_count, 3, "Should have 3 Random segments (30%)");

        // Verify segments are non-overlapping and cover entire range
        let mut last_end = IndexPos::new(0);
        for (range, _) in &segments {
            assert_eq!(range.start, last_end, "Segments should be contiguous");
            assert!(range.end > range.start, "Segments should be non-empty");
            last_end = range.end;
        }
        assert_eq!(
            last_end,
            IndexPos::new(1024 * 10 / 8),
            "Segments should cover entire range"
        );

        // Test reading from each segment
        for (range, bucket) in &segments {
            generator.setup(range.start);
            let val = generator.next_u64();

            match bucket {
                Bucket::Fill => {
                    assert_eq!(val, 0, "Fill segment at {} should return 0", range.start)
                }
                Bucket::Duplicate => {
                    let expected = range.start.as_u64() % 64;
                    assert_eq!(
                        val, expected,
                        "Duplicate segment at {} should match counter pattern",
                        range.start
                    );
                }
                Bucket::Random => {
                    // Just verify it doesn't panic
                }
                Bucket::NotValid => panic!("this should not be seen"),
            }
        }
    }

    #[test]
    fn test_setup_preserves_offset_position() {
        let (mut generator, _) = DataMix::create(
            4096,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 0,
                random: 100,
            },
        );

        // Read value from byte offset 100
        generator.setup(IndexPos::new(100));
        let val_100 = generator.next_u64();

        // Read from other offsets
        for byte_offset in (200..400).step_by(8) {
            generator.setup(IndexPos::new(byte_offset));
            generator.next_u64();
        }

        // Setup at byte offset 100 again
        generator.setup(IndexPos::new(100));
        let val_100_again = generator.next_u64();

        assert_eq!(
            val_100, val_100_again,
            "Same setup should produce same value"
        );
    }

    #[test]
    fn test_sequential_read_across_segments() {
        let (mut generator, segments) = DataMix::create(
            2048,
            12345,
            4,
            &PercentPattern {
                fill: 50,
                duplicates: 25,
                random: 25,
            },
        );

        // Test reading from various byte offsets
        for offset_in_bytes in (0..2000).step_by(96) {
            let index_position = IndexPos::new(offset_in_bytes / 8);

            generator.setup(index_position);
            let val = generator.next_u64();

            // Find which segment we're in
            let segment = segments
                .iter()
                .find(|(range, _)| range.contains(&index_position))
                .unwrap_or_else(|| panic!("No segment for offset {}", index_position));

            let (_range, bucket) = segment;

            // Verify the value matches the pattern
            match bucket {
                Bucket::Fill => assert_eq!(val, 0, "Fill at offset {}", index_position),
                Bucket::Duplicate => {
                    let expected = index_position.as_u64() % 64;
                    assert_eq!(val, expected, "Duplicate at offset {}", index_position);
                }
                Bucket::Random => {}
                Bucket::NotValid => panic!("Should not be seeing NotValid in normal operation"),
            }
        }
    }

    #[test]
    fn test_continuous_sequential_read() {
        // Test reading continuously without setup between reads
        let (mut generator, _) = DataMix::create(
            1024,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // Setup at 0 and read consecutive u64s
        generator.setup(IndexPos::new(0));

        // Read 128 consecutive u64s
        for i in 0..128 {
            let val = generator.next_u64();
            let expected = (i % 64) as u64;
            assert_eq!(val, expected, "Continuous read at index {}", i);
        }
    }

    #[test]
    fn test_random_access_pattern() {
        let (mut generator, _) = DataMix::create(
            4096,
            12345,
            5,
            &PercentPattern {
                fill: 33,
                duplicates: 33,
                random: 34,
            },
        );

        // Test random access
        let test_offsets = vec![0, 800, 1600, 2400, 3200];

        for &byte_offset in &test_offsets {
            // The test offsets are in bytes, not indexes
            let index_offset = IndexPos::new(byte_offset / 8);
            generator.setup(index_offset);
            let val1 = generator.next_u64();

            generator.setup(index_offset);
            let val2 = generator.next_u64();

            assert_eq!(
                val1, val2,
                "Same offset {} should give same value",
                index_offset
            );
        }
    }

    #[test]
    fn test_duplicate_pattern_boundary() {
        let (mut generator, _) = DataMix::create(
            4096,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // Test around the 256-value boundary
        generator.setup(IndexPos::new(254));
        let val_62 = generator.next_u64();
        assert_eq!(val_62, 62);

        generator.setup(IndexPos::new(255));
        let val_63 = generator.next_u64();
        assert_eq!(val_63, 63);

        generator.setup(IndexPos::new(256));
        let val_256 = generator.next_u64();
        assert_eq!(val_256, 0); // Should wrap around

        generator.setup(IndexPos::new(257));
        let val_257 = generator.next_u64();
        assert_eq!(val_257, 1);
    }

    #[test]
    fn test_segment_creation_coverage() {
        // Test various segment counts
        for num_segments in [1, 5, 10, 50, 100] {
            let size = 512 * 5000;
            let (_generator, segments) = DataMix::create(
                size,
                12345,
                num_segments,
                &PercentPattern {
                    fill: 33,
                    duplicates: 33,
                    random: 34,
                },
            );

            assert_eq!(segments.len(), num_segments);

            // Verify complete coverage
            let total_size: u64 = segments.iter().map(|(range, _)| total_bytes(range)).sum();
            assert_eq!(total_size, size, "Segments must cover entire range");
        }
    }

    #[test]
    #[should_panic(expected = "Percentages must sum to 100")]
    fn test_invalid_percentages() {
        DataMix::create(
            1024,
            12345,
            2,
            &PercentPattern {
                fill: 40,
                duplicates: 40,
                random: 40,
            },
        );
    }

    #[test]
    fn test_segment_boundaries() {
        let (mut generator, segments) = DataMix::create(
            2048,
            12345,
            3,
            &PercentPattern {
                fill: 33,
                duplicates: 33,
                random: 34,
            },
        );

        // Test reading at segment boundaries
        for (range, _) in &segments {
            generator.setup(range.start);
            let _val_start = generator.next_u64();

            if range.end.as_u64() >= 8 {
                let last_offset = range.end - IndexPos::new(8);
                generator.setup(IndexPos::new(last_offset));
                let _val_end = generator.next_u64();
            }
        }
    }

    #[test]
    fn test_large_offset() {
        let (mut generator, _) = DataMix::create(
            100352,
            12345,
            1,
            &PercentPattern {
                fill: 0,
                duplicates: 100,
                random: 0,
            },
        );

        // Test with large offsets
        for byte_offset in [0, 10000, 50000, 99000] {
            let index_val = byte_offset / 8;
            generator.setup(IndexPos::new(index_val));
            let val = generator.next_u64();
            let expected = index_val % 64;
            assert_eq!(val, expected, "Duplicate pattern at offset {}", byte_offset);
        }
    }

    #[test]
    fn test_segment_transition() {
        let (mut generator, segments) = DataMix::create(
            2048,
            12345,
            4,
            &PercentPattern {
                fill: 50,
                duplicates: 25,
                random: 25,
            },
        );

        // Test reading across segment boundaries
        for i in segments.iter().skip(1) {
            let boundary = i.0.start;

            if boundary.as_u64() >= 8 {
                // Read before boundary
                generator.setup(IndexPos::new(boundary - IndexPos::new(8)));
                let _val_before = generator.next_u64();

                // Read at boundary
                generator.setup(boundary);
                let _val_at = generator.next_u64();
            }
        }
    }
}
