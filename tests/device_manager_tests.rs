// Integration tests for DeviceManager API
//
// These tests verify the high-level DeviceManager interface for creating,
// managing, and deleting test block devices.

mod common;

use test_bd::{Bucket, DeviceManager, IndexPos, TestBlockDeviceConfig};

#[test]
fn test_device_manager_create_and_delete() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0,              // Explicit device ID assignment
        size: 10 * 1024 * 1024, // 10 MiB
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 50,
        unprivileged: true,
    };

    let device = manager.create(config).expect("Failed to create device");
    assert_eq!(device.dev_id, 0, "Device ID should be 0 as requested");
    assert_eq!(device.segments.len(), 50, "Should have 50 segments");
    assert_eq!(device.config.size, 10 * 1024 * 1024, "Size should match");

    // Verify device exists
    let device_path = format!("/dev/ublkb{}", device.dev_id);
    assert!(
        std::path::Path::new(&device_path).exists(),
        "Device file should exist"
    );

    // Delete device
    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");

    // Verify device is gone
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        !std::path::Path::new(&device_path).exists(),
        "Device file should be removed"
    );
}

#[test]
fn test_device_manager_multiple_devices() {
    let mut manager = DeviceManager::new();

    let config1 = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID
        size: 10 * 1024 * 1024,
        seed: 11111,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 50,
        unprivileged: true,
    };

    let config2 = TestBlockDeviceConfig {
        dev_id: 1, // Explicit device ID
        size: 20 * 1024 * 1024,
        seed: 22222,
        fill_percent: 30,
        duplicate_percent: 40,
        random_percent: 30,
        segments: 100,
        unprivileged: true,
    };

    let device1 = manager.create(config1).expect("Failed to create device 1");
    let device2 = manager.create(config2).expect("Failed to create device 2");

    assert_eq!(device1.dev_id, 0, "Device 1 should have ID 0");
    assert_eq!(device2.dev_id, 1, "Device 2 should have ID 1");

    // List devices
    let devices = manager.list();
    assert_eq!(devices.len(), 2, "Should have 2 managed devices");

    // Verify both devices exist
    assert!(std::path::Path::new(&format!("/dev/ublkb{}", device1.dev_id)).exists());
    assert!(std::path::Path::new(&format!("/dev/ublkb{}", device2.dev_id)).exists());

    // Delete all devices
    manager.delete_all().expect("Failed to delete all devices");

    // Verify all devices are gone
    assert_eq!(
        manager.list().len(),
        0,
        "All devices should be removed from manager"
    );
}

#[test]
fn test_device_manager_auto_cleanup_on_drop() {
    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID
        size: 10 * 1024 * 1024,
        seed: 99999,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 50,
        unprivileged: true,
    };

    let dev_id = {
        let mut manager = DeviceManager::new();
        let device = manager.create(config).expect("Failed to create device");
        let device_path = format!("/dev/ublkb{}", device.dev_id);
        assert!(
            std::path::Path::new(&device_path).exists(),
            "Device should exist"
        );
        device.dev_id
        // manager drops here, should auto-cleanup
    };

    // Give the cleanup some time
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Verify device is gone after manager drop
    let device_path = format!("/dev/ublkb{}", dev_id);
    assert!(
        !std::path::Path::new(&device_path).exists(),
        "Device should be cleaned up when manager is dropped"
    );
}

#[test]
fn test_segment_info_verification() {
    let mut manager = DeviceManager::new();

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID
        size: 10 * 1024 * 1024,
        seed: 42,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };

    let device = manager
        .create(config.clone())
        .expect("Failed to create device");

    // Verify segments cover the entire device
    let mut total_size = 0u64;
    let mut last_end = IndexPos::new(0);

    for segment in &device.segments {
        assert_eq!(segment.start, last_end, "Segments should be contiguous");
        assert!(segment.end > segment.start, "Segment end should be > start");
        assert_eq!(segment.count(), segment.end - segment.start);
        total_size += segment.size_bytes();
        last_end = segment.end;
    }

    assert_eq!(
        total_size, config.size,
        "Segments should cover entire device"
    );
    assert_eq!(
        last_end,
        IndexPos::new(config.size / 8),
        "Last segment should end at device size"
    );

    // Count segment types
    let fill_count = device
        .segments
        .iter()
        .filter(|s| s.pattern == Bucket::Fill)
        .count();
    let dup_count = device
        .segments
        .iter()
        .filter(|s| s.pattern == Bucket::Duplicate)
        .count();
    let rand_count = device
        .segments
        .iter()
        .filter(|s| s.pattern == Bucket::Random)
        .count();

    assert_eq!(
        fill_count + dup_count + rand_count,
        100,
        "Should have 100 total segments"
    );

    // Percentages should be roughly correct (within rounding)
    let expected_fill = 25;
    let expected_dup = 50;
    let expected_rand = 25;

    assert!(
        (fill_count as i32 - expected_fill).abs() <= 1,
        "Fill segments should be close to 25%, got {}",
        fill_count
    );
    assert!(
        (dup_count as i32 - expected_dup).abs() <= 1,
        "Duplicate segments should be close to 50%, got {}",
        dup_count
    );
    assert!(
        (rand_count as i32 - expected_rand).abs() <= 1,
        "Random segments should be close to 25%, got {}",
        rand_count
    );

    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
}

#[test]
fn test_generate_segments_without_creating_device() {
    let config = TestBlockDeviceConfig {
        dev_id: -1,
        size: 10 * 1024 * 1024,
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };

    // Generate segments without creating device
    let segments = config.generate_segments();

    assert_eq!(segments.len(), 100, "Should generate 100 segments");

    // Verify segments are valid
    let mut total_size = 0u64;
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            assert_eq!(
                segment.start,
                segments[i - 1].end,
                "Segments should be contiguous"
            );
        }
        total_size += segment.size_bytes();
    }

    assert_eq!(
        total_size, config.size,
        "Segments should cover entire device"
    );
}

#[test]
fn test_config_validation() {
    // Valid config
    let valid_config = TestBlockDeviceConfig {
        dev_id: -1,
        size: 10 * 1024 * 1024,
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };
    assert!(valid_config.validate().is_ok());

    // Invalid percentages (don't sum to 100)
    let invalid_percent_config = TestBlockDeviceConfig {
        dev_id: -1,
        size: 10 * 1024 * 1024,
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 30, // Sum is 105
        segments: 100,
        unprivileged: true,
    };
    assert!(invalid_percent_config.validate().is_err());

    // Too many segments
    let invalid_segments_config = TestBlockDeviceConfig {
        dev_id: -1,
        size: 1024, // 1 KiB
        seed: 12345,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 10000, // More than size/512
        unprivileged: true,
    };
    assert!(invalid_segments_config.validate().is_err());
}
