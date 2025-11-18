// Integration tests for device size limits
//
// These tests verify that the library correctly handles minimum and maximum
// block device sizes, including edge cases and validation.

use std::fs::File;
use std::io::Read;
use test_bd::{DeviceManager, TestBlockDeviceConfig};

#[test]
fn test_minimum_device_size() {
    let mut manager = DeviceManager::new();

    // Minimum viable size: Must be large enough for at least 1 segment
    // and segments must be at least 512 bytes based on the constraint:
    // segments < device_size / 512
    let min_size = 1024u64; // 1 KiB - allows for 1 segment

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: min_size,
        seed: 12345,
        fill_percent: 100,
        duplicate_percent: 0,
        random_percent: 0,
        segments: 1, // Minimum segments
        unprivileged: true,
    };

    config.validate().expect("Minimum config should be valid");

    let device = manager
        .create(config)
        .expect("Failed to create minimum size device");
    assert_eq!(device.config.size, min_size);
    assert_eq!(device.segments.len(), 1);
    assert_eq!(device.segments[0].size_bytes(), min_size);

    // Verify device is usable
    let device_path = format!("/dev/ublkb{}", device.dev_id);
    let mut file = File::open(&device_path).expect("Failed to open device");

    let mut buffer = vec![0u8; min_size as usize];
    file.read_exact(&mut buffer)
        .expect("Failed to read from minimum size device");

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_small_device_sizes() {
    let mut manager = DeviceManager::new();

    let test_sizes = vec![
        512 * 2,    // 1 KiB
        512 * 4,    // 2 KiB
        512 * 8,    // 4 KiB (typical page size)
        512 * 16,   // 8 KiB
        512 * 1024, // 512 KiB
    ];

    for size in test_sizes {
        // Calculate max segments for this size (must be < size / 512)
        let max_segments = (size / 512u64).saturating_sub(1).max(1);

        let config = TestBlockDeviceConfig {
            dev_id: 0, // Explicit device ID assignment
            size,
            seed: size, // Use size as seed for variety
            fill_percent: 100,
            duplicate_percent: 0,
            random_percent: 0,
            segments: max_segments as usize,
            unprivileged: true,
        };

        config
            .validate()
            .unwrap_or_else(|_| panic!("Config for size {} should be valid", size));

        let device = manager
            .create(config)
            .unwrap_or_else(|_| panic!("Failed to create device of size {}", size));
        assert_eq!(device.config.size, size);

        // Verify device is readable
        let device_path = format!("/dev/ublkb{}", device.dev_id);
        let mut file = File::open(&device_path).expect("Failed to open device");
        let mut buffer = vec![0u8; size as usize];
        file.read_exact(&mut buffer)
            .unwrap_or_else(|_| panic!("Failed to read device of size {}", size));

        drop(file);

        manager
            .delete(device.dev_id)
            .expect("Failed to delete device");
    }
}

#[test]
fn test_medium_device_sizes() {
    let mut manager = DeviceManager::new();

    let test_sizes = vec![
        1024 * 1024,       // 1 MiB
        10 * 1024 * 1024,  // 10 MiB
        100 * 1024 * 1024, // 100 MiB
        512 * 1024 * 1024, // 512 MiB
    ];

    for size in test_sizes {
        let config = TestBlockDeviceConfig {
            dev_id: 0, // Explicit device ID assignment
            size,
            seed: 42,
            fill_percent: 25,
            duplicate_percent: 50,
            random_percent: 25,
            segments: 100,
            unprivileged: true,
        };

        config
            .validate()
            .unwrap_or_else(|_| panic!("Config for size {} should be valid", size));

        let device = manager
            .create(config)
            .unwrap_or_else(|_| panic!("Failed to create device of size {}", size));
        assert_eq!(device.config.size, size);

        // Just verify the device exists and segments are correct
        let total_segment_size: u64 = device.segments.iter().map(|s| s.size_bytes()).sum();
        assert_eq!(
            total_segment_size, size,
            "Segments should cover entire device of size {}",
            size
        );

        manager
            .delete(device.dev_id)
            .expect("Failed to delete device");
    }
}

#[test]
fn test_large_device_size() {
    let mut manager = DeviceManager::new();

    // Test 1 GiB device
    let size = 1024u64 * 1024 * 1024;

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size,
        seed: 99999,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 1000,
        unprivileged: true,
    };

    config.validate().expect("1 GiB config should be valid");

    let device = manager
        .create(config)
        .expect("Failed to create 1 GiB device");
    assert_eq!(device.config.size, size);
    assert_eq!(device.segments.len(), 1000);

    // Verify segments cover entire device
    let total_segment_size: u64 = device.segments.iter().map(|s| s.size_bytes()).sum();
    assert_eq!(total_segment_size, size);

    // Verify device is accessible
    let device_path = format!("/dev/ublkb{}", device.dev_id);
    assert!(std::path::Path::new(&device_path).exists());

    // Read from a few locations to verify it works
    let mut file = File::open(&device_path).expect("Failed to open 1 GiB device");

    // Read from start
    let mut buffer = vec![0u8; 4096];
    file.read_exact(&mut buffer)
        .expect("Failed to read from start");

    // Read from middle (seek and read)
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(size / 2))
        .expect("Failed to seek");
    file.read_exact(&mut buffer)
        .expect("Failed to read from middle");

    drop(file);

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_very_large_device_size() {
    // Test 10 GiB device (but don't actually read all the data)
    let size = 10u64 * 1024 * 1024 * 1024;

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size,
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 5000,
        unprivileged: true,
    };

    config.validate().expect("10 GiB config should be valid");

    // Just verify segments can be generated
    let segments = config.generate_segments();
    assert_eq!(segments.len(), 5000);

    let total_size: u64 = segments.iter().map(|s| s.size_bytes()).sum();
    assert_eq!(total_size, size);

    // Note: We don't actually create this device to keep test time reasonable
    // The validation and segment generation is the key test here
}

#[test]
fn test_maximum_safe_device_size() {
    let max_size = i64::MAX - (i64::MAX % 8); // We require the block size to be multiples of 8 bytes

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: max_size as u64,
        seed: 42,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 1000000, // 1 million segments for a huge device
        unprivileged: true,
    };

    // Should validate successfully
    config.validate().expect("Maximum size should be valid");

    // Verify we can generate segments (but don't create the device)
    let segments = config.generate_segments();
    assert_eq!(segments.len(), 1000000);

    let total_size: u64 = segments.iter().map(|s| s.size_bytes()).sum();
    assert_eq!(total_size, max_size as u64);
}

#[test]
fn test_size_too_large_validation() {
    // Size larger than i64::MAX should fail validation
    let too_large = (i64::MAX as u64) + 1;

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: too_large,
        seed: 42,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 1000,
        unprivileged: true,
    };

    // Should fail validation
    assert!(
        config.validate().is_err(),
        "Size exceeding i64::MAX should fail validation"
    );
}

#[test]
fn test_too_many_segments_validation() {
    let size = 10 * 1024 * 1024; // 10 MiB

    // Try to create more segments than size/512 allows
    let max_segments = (size / 512) as usize;
    let too_many_segments = max_segments + 1;

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size,
        seed: 42,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: too_many_segments,
        unprivileged: true,
    };

    // Should fail validation
    assert!(
        config.validate().is_err(),
        "Too many segments should fail validation"
    );
}

#[test]
fn test_segment_count_limits() {
    let mut manager = DeviceManager::new();
    let size = 100 * 1024 * 1024; // 100 MiB

    let test_segment_counts = vec![
        1, // Minimum
        10,
        100,
        1000,
        10000,
        ((size / 512) - 1) as usize, // Maximum allowed
    ];

    for segments in test_segment_counts {
        let config = TestBlockDeviceConfig {
            dev_id: 0, // Explicit device ID assignment
            size,
            seed: segments as u64, // Different seed for each
            fill_percent: 25,
            duplicate_percent: 50,
            random_percent: 25,
            segments,
            unprivileged: true,
        };

        config
            .validate()
            .unwrap_or_else(|_| panic!("Config with {} segments should be valid", segments));

        let device = manager
            .create(config)
            .unwrap_or_else(|_| panic!("Failed to create device with {} segments", segments));
        assert_eq!(device.segments.len(), segments);

        // Verify all segments are contiguous and cover entire device
        let mut last_end = 0u64;
        for segment in &device.segments {
            assert_eq!(
                segment.start.as_abs_byte_offset(),
                last_end,
                "Segments should be contiguous"
            );
            last_end = segment.end.as_abs_byte_offset();
        }
        assert_eq!(last_end, size, "Segments should cover entire device");

        manager
            .delete(device.dev_id)
            .expect("Failed to delete device");
    }
}
