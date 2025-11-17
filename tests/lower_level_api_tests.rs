// Integration tests for lower-level TestBlockDevice API
//
// These tests verify the TestBlockDevice static methods for
// device creation and deletion without using DeviceManager.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use test_bd::{Bucket, IndexPos, SegmentInfo, TestBlockDevice, TestBlockDeviceConfig};

#[test]
fn test_run_with_callback() {
    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 54321,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 50,
        unprivileged: true,
    };

    let callback_called = Arc::new(Mutex::new(false));
    let callback_called_clone = callback_called.clone();
    let segments_received = Arc::new(Mutex::new(Vec::new()));
    let segments_received_clone = segments_received.clone();
    let dev_id_received = Arc::new(Mutex::new(-1));
    let dev_id_received_clone = dev_id_received.clone();

    // Spawn device in thread
    let handle = thread::spawn(move || {
        TestBlockDevice::run_with_callback(
            config,
            Some(move |dev_id, segments| {
                *callback_called_clone.lock().unwrap() = true;
                *dev_id_received_clone.lock().unwrap() = dev_id;
                *segments_received_clone.lock().unwrap() = segments;
            }),
        )
    });

    // Wait for callback to be called
    for _ in 0..50 {
        if *callback_called.lock().unwrap() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    assert!(
        *callback_called.lock().unwrap(),
        "Callback should be called"
    );

    let dev_id = *dev_id_received.lock().unwrap();
    assert!(dev_id >= 0, "Device ID should be valid");

    let segments = segments_received.lock().unwrap();
    assert_eq!(segments.len(), 50, "Should have 50 segments");

    // Verify device exists
    let device_path = format!("/dev/ublkb{}", dev_id);
    assert!(
        std::path::Path::new(&device_path).exists(),
        "Device should exist"
    );

    // Delete device
    TestBlockDevice::delete(dev_id, false).expect("Failed to delete device");

    // Wait for thread to finish
    handle.join().expect("Thread panicked").ok();
}

#[test]
fn test_delete_device() {
    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 5 * 1024 * 1024,
        seed: 11111,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 25,
        unprivileged: true,
    };

    let dev_id_arc = Arc::new(Mutex::new(-1));
    let dev_id_clone = dev_id_arc.clone();

    let handle = thread::spawn(move || {
        TestBlockDevice::run_with_callback(
            config,
            Some(move |dev_id, _| {
                *dev_id_clone.lock().unwrap() = dev_id;
            }),
        )
    });

    // Wait for device to be ready
    let dev_id = loop {
        let id = *dev_id_arc.lock().unwrap();
        if id >= 0 {
            break id;
        }
        thread::sleep(Duration::from_millis(100));
    };

    // Verify device exists
    let device_path = format!("/dev/ublkb{}", dev_id);
    assert!(std::path::Path::new(&device_path).exists());

    // Test synchronous delete
    TestBlockDevice::delete(dev_id, false).expect("Failed to delete device (sync)");

    // Wait for device to be removed
    thread::sleep(Duration::from_millis(100));
    assert!(
        !std::path::Path::new(&device_path).exists(),
        "Device should be removed after delete"
    );

    // Thread should exit after delete
    handle.join().expect("Thread panicked").ok();
}

#[test]
fn test_callback_receives_correct_segments() {
    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 99999,
        fill_percent: 20,
        duplicate_percent: 30,
        random_percent: 50,
        segments: 100,
        unprivileged: true,
    };

    // Generate expected segments
    let expected_segments = config.generate_segments();

    let received_segments = Arc::new(Mutex::new(Vec::new()));
    let received_segments_clone = received_segments.clone();
    let dev_id_arc = Arc::new(Mutex::new(-1));
    let dev_id_clone = dev_id_arc.clone();

    let handle = thread::spawn(move || {
        TestBlockDevice::run_with_callback(
            config,
            Some(move |dev_id, segments| {
                *dev_id_clone.lock().unwrap() = dev_id;
                *received_segments_clone.lock().unwrap() = segments;
            }),
        )
    });

    // Wait for callback
    let dev_id = loop {
        let id = *dev_id_arc.lock().unwrap();
        if id >= 0 {
            break id;
        }
        thread::sleep(Duration::from_millis(100));
    };

    let received = received_segments.lock().unwrap();
    assert_eq!(
        received.len(),
        expected_segments.len(),
        "Received segments should match expected count"
    );

    // Compare each segment
    for (i, (received_seg, expected_seg)) in
        received.iter().zip(expected_segments.iter()).enumerate()
    {
        assert_eq!(
            received_seg.start, expected_seg.start,
            "Segment {} start should match",
            i
        );
        assert_eq!(
            received_seg.end, expected_seg.end,
            "Segment {} end should match",
            i
        );
        assert_eq!(
            received_seg.pattern, expected_seg.pattern,
            "Segment {} pattern should match",
            i
        );
    }

    // Cleanup
    TestBlockDevice::delete(dev_id, false).expect("Failed to delete device");
    handle.join().expect("Thread panicked").ok();
}

#[test]
fn test_segment_info_size_method() {
    let segment = SegmentInfo {
        start: IndexPos::new(1024),
        end: IndexPos::new(2048),
        pattern: Bucket::Fill,
    };

    assert_eq!(segment.count(), 1024, "Count calculation should be correct");

    let segment2 = SegmentInfo {
        start: IndexPos::new(0),
        end: IndexPos::new(1024 * 1024),
        pattern: Bucket::Random,
    };

    assert_eq!(segment2.size_bytes(), 1024 * 1024 * 8);
}

#[test]
fn test_different_seeds_produce_different_segments() {
    let config1 = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 11111,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };

    let config2 = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 22222, // Different seed
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };

    let segments1 = config1.generate_segments();
    let segments2 = config2.generate_segments();

    // Same number of segments
    assert_eq!(segments1.len(), segments2.len());

    // But different layouts (at least some segments should differ)
    let differences = segments1
        .iter()
        .zip(segments2.iter())
        .filter(|(s1, s2)| s1.start != s2.start || s1.end != s2.end || s1.pattern != s2.pattern)
        .count();

    assert!(
        differences > 0,
        "Different seeds should produce different segment layouts"
    );
}

#[test]
fn test_same_seed_produces_same_segments() {
    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 10 * 1024 * 1024,
        seed: 42424242,
        fill_percent: 25,
        duplicate_percent: 50,
        random_percent: 25,
        segments: 100,
        unprivileged: true,
    };

    let segments1 = config.generate_segments();
    let segments2 = config.generate_segments();

    assert_eq!(segments1.len(), segments2.len());

    // All segments should be identical
    for (s1, s2) in segments1.iter().zip(segments2.iter()) {
        assert_eq!(s1.start, s2.start);
        assert_eq!(s1.end, s2.end);
        assert_eq!(s1.pattern, s2.pattern);
    }
}
