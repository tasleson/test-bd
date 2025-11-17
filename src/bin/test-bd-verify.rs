use clap::Parser;
use colored::*;

use rand::{RngCore, SeedableRng};
use rand_pcg::Mcg128Xsl64;
use serde::Deserialize;
use std::fs::File;
use std::io::{self, BufReader, Read};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
enum Pattern {
    Fill,
    Random,
    Duplicate,
}

#[derive(Debug, Deserialize)]
struct Segment {
    start: u64, // in 8-byte units
    end: u64,   // non-inclusive, in 8-byte units
    pattern: Pattern,
}

#[derive(Debug, Deserialize)]
struct TestBdMeta {
    seed: u64,
    #[allow(dead_code)]
    device_id: u64,
    segments: Vec<Segment>,
}

#[derive(Parser)]
struct Args {
    /// Path to file or block device
    path: String,

    /// JSON manifest
    manifest: String,
}

fn find_segment(segments: &[Segment], abs_byte: u64) -> Option<&Segment> {
    let word = abs_byte / 8;
    segments.iter().find(|s| s.start <= word && word < s.end)
}

fn expected_random_byte(seed: u64, abs_off: u64) -> u8 {
    let word_index = abs_off / 8;
    let byte_in_word = (abs_off % 8) as usize;

    let mut rng = Mcg128Xsl64::seed_from_u64(seed);
    rng.advance(word_index as u128); // EXACTLY like set_state_to

    let be = rng.next_u64().to_be_bytes();
    be[byte_in_word]
}

fn expected_byte_for(manifest: &TestBdMeta, segment: &Segment, abs_off: u64) -> u8 {
    match segment.pattern {
        Pattern::Fill => 0x00,

        Pattern::Duplicate => {
            let word_index = abs_off / 8;
            let expected_u64 = word_index % 64;
            let be = expected_u64.to_be_bytes();
            be[(abs_off % 8) as usize]
        }

        Pattern::Random => expected_random_byte(manifest.seed, abs_off),
    }
}

fn expected_line_for(
    manifest: &TestBdMeta,
    segments: &[Segment],
    abs_off: u64,
    filled: usize,
    actual: &[u8],
) -> (Option<[u8; 16]>, bool) {
    let mut expected = [0u8; 16];
    let mut any_mismatch = false;

    for i in 0..filled {
        let off = abs_off + i as u64;
        if let Some(seg) = find_segment(segments, off) {
            let exp = expected_byte_for(manifest, seg, off);
            expected[i] = exp;
            if exp != actual[i] {
                any_mismatch = true;
            }
        } else {
            // Outside any segment: treat as mismatch with expected=0 for display
            expected[i] = 0;
            if actual[i] != 0 {
                any_mismatch = true;
            }
        }
    }

    if any_mismatch {
        (Some(expected), true)
    } else {
        (None, false)
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    let manifest_file = File::open(&args.manifest)?;
    let manifest: TestBdMeta = serde_json::from_reader(manifest_file)?;

    let file = File::open(&args.path)?;
    let mut reader = BufReader::new(file);

    let mut buf = [0u8; 16];
    let mut abs_off: u64 = 0;

    loop {
        let mut filled = 0;
        while filled < buf.len() {
            match reader.read(&mut buf[filled..])? {
                0 if filled == 0 => return Ok(()),
                0 => break,
                n => filled += n,
            }
        }

        let offset_words = abs_off / 8;
        print!("{:08}: ", offset_words);

        // First, compute expected bytes and mismatch flags.
        let (expected_opt, any_mismatch) =
            expected_line_for(&manifest, &manifest.segments, abs_off, filled, &buf);

        // Print actual bytes with colors.
        //for i in 0..filled {
        for (i, das_byte) in buf.iter().enumerate().take(filled) {
            let off = abs_off + i as u64;
            let seg_opt = find_segment(&manifest.segments, off);

            let (_expected, is_match) = if let Some(seg) = seg_opt {
                let expected = expected_byte_for(&manifest, seg, off);
                (expected, expected == *das_byte)
            } else {
                (0u8, *das_byte == 0)
            };

            let cs = if is_match {
                format!("{:02x}", das_byte).green()
            } else {
                format!("{:02x}", das_byte).red()
            };

            if i == 0 {
                print!("{}", cs);
            } else if i == 8 {
                print!("  {}", cs);
            } else {
                print!(" {}", cs);
            }
        }

        // If any mismatch on this line, print expected column on the right.
        if any_mismatch {
            if let Some(expected) = expected_opt {
                print!(" | expected:");
                for (i, the_byte) in expected.iter().enumerate().take(filled) {
                    let b = the_byte;
                    let s = format!("{:02x}", b).green();
                    if i == 0 {
                        print!(" {}", s);
                    } else if i == 8 {
                        print!("  {}", s);
                    } else {
                        print!(" {}", s);
                    }
                }
            }
        }

        println!();
        abs_off += filled as u64;
    }
}
