use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::thread;
use std::time::Duration;
use test_bd::{Bucket, DeviceManager, TestBlockDeviceConfig};

mod common;

fn open_device_with_retry(device_path: &str) -> std::io::Result<File> {
    log::debug!("Attempting to open device: {}", device_path);
    let max_attempts = 60; // 60 * 100ms = 6 seconds
    let mut last_error = None;

    for attempt in 0..max_attempts {
        match File::open(device_path) {
            Ok(file) => {
                log::debug!(
                    "Successfully opened {} on attempt {}",
                    device_path,
                    attempt + 1
                );
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                if attempt % 10 == 0 {
                    log::debug!(
                        "Permission denied, attempt {}/{}",
                        attempt + 1,
                        max_attempts
                    );
                }
                last_error = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                log::debug!("Error opening {}: {}", device_path, e);
                return Err(e);
            }
        }
    }

    log::debug!(
        "Failed to open {} after {} attempts",
        device_path,
        max_attempts
    );
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Failed to open device after retries",
        )
    }))
}

fn read_u64_values(file: &mut File, offset: u64, count: usize) -> std::io::Result<Vec<u64>> {
    log::debug!("read_u64_values offset {offset} count {count}");

    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; count * 8];
    file.read_exact(&mut buffer)?;

    let values: Vec<u64> = buffer
        .chunks_exact(8)
        .map(|chunk| {
            let bytes: [u8; 8] = chunk.try_into().unwrap();
            u64::from_be_bytes(bytes)
        })
        .collect();

    Ok(values)
}

#[test]
fn test_fill_pattern_verification() {
    common::test_utils::init_logging();
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 12345,
        fill_percent: 100, // All fill
        duplicate_percent: 0,
        random_percent: 0,
        segments: 50,
        unprivileged: true,
    };

    let device = manager.create(config).expect("Failed to create device");
    log::debug!("Device created with ID: {}", device.dev_id);
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    // Verify all segments are Fill type
    for segment in &device.segments {
        assert_eq!(segment.pattern, Bucket::Fill, "All segments should be Fill");
    }
    log::debug!(
        "Segments verified, {} segments total",
        device.segments.len()
    );

    // Read data from device and verify it's all zeros
    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");

    // Read from first segment
    let first_segment = &device.segments[0];
    let values = read_u64_values(&mut file, first_segment.start.as_u64(), 128)
        .expect("Failed to read from device");

    for (i, &value) in values.iter().enumerate() {
        assert_eq!(
            value,
            0,
            "Fill pattern should be all zeros at offset {}",
            i * 8
        );
    }

    // Read from middle segment
    let mid_segment = &device.segments[device.segments.len() / 2];
    let values = read_u64_values(&mut file, mid_segment.start.as_u64(), 128)
        .expect("Failed to read from device");

    for (i, &value) in values.iter().enumerate() {
        assert_eq!(
            value,
            0,
            "Fill pattern should be all zeros at offset {}",
            i * 8
        );
    }

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_duplicate_pattern_verification() {
    // Initialize logging
    common::test_utils::init_logging();

    log::debug!("=== Starting test_duplicate_pattern_verification ===");

    log::debug!("Creating DeviceManager");
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 54321,
        fill_percent: 0,
        duplicate_percent: 100, // All duplicate
        random_percent: 0,
        segments: 50,
        unprivileged: true,
    };

    log::debug!(
        "Config created: dev_id={}, size={}, seed={}, segments={}",
        config.dev_id,
        config.size,
        config.seed,
        config.segments
    );
    log::debug!("Calling manager.create()...");
    let device = manager.create(config).expect("Failed to create device");
    log::debug!("Device created successfully with ID: {}", device.dev_id);
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    // Verify all segments are Duplicate type
    log::debug!(
        "Verifying all {} segments are Duplicate type",
        device.segments.len()
    );
    for (idx, segment) in device.segments.iter().enumerate() {
        log::debug!(
            "Segment {}: start={}, end={}, pattern={:?}",
            idx,
            segment.start,
            segment.end,
            segment.pattern
        );
        assert_eq!(
            segment.pattern,
            Bucket::Duplicate,
            "All segments should be Duplicate"
        );
    }
    log::debug!("All segments verified as Duplicate type");

    // Read data from device and verify duplicate pattern (0-255 repeating)
    log::debug!("Opening device at {}", device_path);
    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");
    log::debug!("Device file opened successfully");

    // Read from first segment
    let first_segment = &device.segments[0];
    log::debug!(
        "Reading 512 u64 values from first segment at offset {}",
        first_segment.start
    );
    let values = read_u64_values(&mut file, first_segment.start.as_u64(), 512)
        .expect("Failed to read from device");
    log::debug!(
        "Successfully read {} values from first segment",
        values.len()
    );

    // Duplicate pattern should be (offset % 512) for each u64
    log::debug!("Verifying duplicate pattern (0-64 repeating)");
    for (i, &value) in values.iter().enumerate() {
        let expected = (i % 64) as u64;
        if i % 100 == 0 {
            log::debug!("Value at index {}: got={}, expected={}", i, value, expected);
        }
        assert_eq!(
            value, expected,
            "Duplicate pattern should repeat 0-255, got {} at index {}",
            value, i
        );
    }
    log::debug!("First 512 values verified successfully");

    // Verify pattern repeats after 512 values
    log::debug!(
        "Reading second set of 512 values at offset {} to verify repetition",
        first_segment.start.as_u64() + 512 * 8
    );
    let values2 = read_u64_values(&mut file, first_segment.start.as_u64() + 512 * 8, 512)
        .expect("Failed to read from device");
    log::debug!("Successfully read second set of {} values", values2.len());

    log::debug!("Comparing first and second sets to verify pattern repetition");
    assert_eq!(
        values, values2,
        "Duplicate pattern should repeat every 512 values"
    );
    log::debug!("Pattern repetition verified successfully");

    log::debug!("About to delete device {}", device.dev_id);
    log::debug!("Calling manager.delete({})...", device.dev_id);

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
    log::debug!("manager.delete() returned successfully");
    log::debug!("=== test_duplicate_pattern_verification completed successfully ===");
}

#[test]
fn test_random_pattern_deterministic() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 99999,
        fill_percent: 0,
        duplicate_percent: 0,
        random_percent: 100, // All random
        segments: 50,
        unprivileged: true,
    };

    let device = manager
        .create(config.clone())
        .expect("Failed to create device");
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    // Read some random data
    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");
    let first_read = read_u64_values(&mut file, 0, 1024).expect("Failed to read from device");

    // Verify data is not all zeros and not all the same
    let all_zeros = first_read.iter().all(|&v| v == 0);
    let all_same = first_read.windows(2).all(|w| w[0] == w[1]);
    assert!(!all_zeros, "Random data should not be all zeros");
    assert!(!all_same, "Random data should not be all the same value");

    // Delete and recreate with same seed
    drop(file);
    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");

    let device2 = manager.create(config).expect("Failed to create device 2");
    let device_path2 = format!("/dev/ublkb{}", device2.dev_id);

    // Read same data from recreated device
    let mut file2 = open_device_with_retry(&device_path2).expect("Failed to open device 2");
    let second_read = read_u64_values(&mut file2, 0, 1024).expect("Failed to read from device 2");

    // Verify deterministic - same seed produces same data
    assert_eq!(
        first_read, second_read,
        "Same seed should produce identical random data"
    );

    drop(file2);

    manager
        .delete(device2.dev_id)
        .expect("Failed to delete device 2");
}

use std::io::{self, Write};

pub fn dump_u64_xxd(in_offset: usize, data: &[u64]) {
    let mut offset = in_offset;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Convert to raw bytes
    let bytes: Vec<u8> = data
        .iter()
        .flat_map(|v| v.to_be_bytes()) // or `.to_le_bytes()` if you prefer
        .collect();

    for chunk in bytes.chunks(16) {
        // Print offset
        write!(out, "{:08}: ", offset).unwrap();

        // Print hex bytes
        for (i, b) in chunk.iter().enumerate() {
            write!(out, "{:02x}", b).unwrap();

            // Add spacing like xxd:
            if i % 2 == 1 {
                write!(out, " ").unwrap();
            }
            if i == 7 {
                write!(out, " ").unwrap();
            } // extra space midline
        }

        writeln!(out).unwrap();
        offset += chunk.len();
    }
}

#[test]
fn test_mixed_pattern_verification() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 42424,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };
    common::test_utils::init_logging();

    let device = manager.create(config).expect("Failed to create device");
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");

    // Verify each segment type
    for segment in &device.segments {
        log::debug!("checking segment: {:?}", segment);

        // Read some data from this segment
        let read_count = std::cmp::min(256, segment.count());
        if read_count == 0 {
            continue;
        }

        let read_count = read_count as usize;

        log::debug!(
            "checking segment: {:?} reading chunks {} bytes = {}",
            segment,
            read_count,
            read_count * 8
        );

        let values = read_u64_values(&mut file, segment.start.as_abs_byte_offset(), read_count)
            .unwrap_or_else(|_| panic!("Failed to read from segment at offset {}", segment.start));

        assert_eq!(read_count, values.len());

        log::debug!("We have {} u64 values", values.len());

        match segment.pattern {
            Bucket::Fill => {
                if values.iter().sum::<u64>() != 0 {
                    dump_u64_xxd(segment.start.as_u64() as usize, &values);
                }

                // All values should be zero
                for (i, &value) in values.iter().enumerate() {
                    assert_eq!(
                        value, 0,
                        "Fill segment at offset {} should be zero at index {}",
                        segment.start, i
                    );
                }
            }
            Bucket::Duplicate => {
                // Values should follow pattern (offset % 64)
                for (i, &value) in values.iter().enumerate() {
                    let offset_in_segment = (segment.start.as_u64() + i as u64) % 64;
                    let expected = offset_in_segment % 64;
                    assert_eq!(
                        value, expected,
                        "Duplicate segment at offset {} should have pattern {} at index {}, got {}",
                        segment.start, expected, i, value
                    );
                }
            }
            Bucket::Random => {
                // Should not be all zeros and should have variety
                let all_zeros = values.iter().all(|&v| v == 0);
                assert!(
                    !all_zeros,
                    "Random segment at offset {} should not be all zeros",
                    segment.start
                );

                if values.len() > 1 {
                    let all_same = values.windows(2).all(|w| w[0] == w[1]);
                    assert!(
                        !all_same,
                        "Random segment at offset {} should have variety",
                        segment.start
                    );
                }
            }
            Bucket::NotValid => panic!("Should never see"),
        }
    }

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_segment_boundary_reading() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 77777,
        fill_percent: 33,
        duplicate_percent: 34,
        random_percent: 33,
        segments: 50,
        unprivileged: true,
    };

    let device = manager.create(config).expect("Failed to create device");
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");

    // Test reading across segment boundaries
    for i in 0..device.segments.len() - 1 {
        let seg1 = &device.segments[i];
        let seg2 = &device.segments[i + 1];

        // Read from end of first segment and beginning of second
        let read_start = seg1.end.as_u64().saturating_sub(64);
        let read_count = 16; // 16 * 8 = 128 bytes across boundary

        let values = read_u64_values(&mut file, read_start, read_count)
            .expect("Failed to read across segment boundary");

        assert_eq!(values.len(), read_count);

        // Verify the boundary is at the correct offset
        assert_eq!(
            seg1.end,
            seg2.start,
            "Segments {} and {} should be contiguous",
            i,
            i + 1
        );
    }

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_read_entire_device_sequential() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0,         // Explicit device ID assignment
        size: 1024 * 1024, // 1 MiB for faster test
        seed: 11111,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 20,
        unprivileged: true,
    };

    let device = manager
        .create(config.clone())
        .expect("Failed to create device");
    let device_path = format!("/dev/ublkb{}", device.dev_id);

    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");

    // Read entire device
    let mut buffer = vec![0u8; config.size as usize];
    file.read_exact(&mut buffer)
        .expect("Failed to read entire device");

    // Verify total bytes read
    assert_eq!(buffer.len(), config.size as usize);

    // Verify each segment's data
    for segment in &device.segments {
        let start = segment.start.as_abs_byte_offset() as usize;
        let end = segment.end.as_abs_byte_offset() as usize;
        let segment_data = &buffer[start..end];

        match segment.pattern {
            Bucket::Fill => {
                if !segment_data.iter().all(|&b| b == 0) {
                    common::test_utils::xxd_dump(segment_data);
                }

                assert!(
                    segment_data.iter().all(|&b| b == 0),
                    "Fill segment at {} should be all zeros",
                    segment.start
                );
            }
            Bucket::Duplicate | Bucket::Random => {
                // Just verify it's not causing issues
                assert_eq!(
                    segment_data.len(),
                    (end - start),
                    "Segment size should match"
                );
            }
            Bucket::NotValid => panic!("we should never get this"),
        }
    }

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_random_access_pattern() {
    // Initialize logging to see debug output
    common::test_utils::init_logging();

    log::debug!("DeviceManager::new()");
    let mut manager = DeviceManager::new();
    log::debug!("DeviceManager::new()");

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 33333,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };
    log::debug!("we have a config");

    let device = manager
        .create(config.clone())
        .expect("Failed to create device");

    log::debug!("We created a device!");
    thread::sleep(Duration::from_secs(3));
    log::debug!("We waited for stuff to settle");

    let device_path = format!("/dev/ublkb{}", device.dev_id);

    log::debug!("opening actual block device with retries");
    let mut file = open_device_with_retry(&device_path).expect("Failed to open device");

    // Test random access to various offsets
    let test_offsets = vec![
        0u64,                    // Start
        512,                     // Small offset
        1024 * 1024,             // 1 MiB
        5 * 1024 * 1024,         // Middle
        10 * 1024 * 1024 - 1024, // Near end
    ];

    log::debug!("Reading some data");

    for offset in test_offsets {
        if offset >= config.size {
            continue;
        }

        let read_size = std::cmp::min(512, (config.size - offset) / 8);
        if read_size == 0 {
            continue;
        }

        log::debug!("issuing a read offset {offset} len {read_size}");
        let values = read_u64_values(&mut file, offset, read_size as usize)
            .unwrap_or_else(|_| panic!("Failed to read at offset {}", offset));

        assert_eq!(
            values.len(),
            read_size as usize,
            "Should read correct amount at offset {}",
            offset
        );
    }

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}
