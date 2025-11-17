// Stress tests for test-bd
//
// These tests push the system to its limits to find resource constraints.
// They may take longer to run and consume more resources than regular tests.

mod common;

use test_bd::{DeviceManager, TestBlockDeviceConfig};

#[test]
fn test_max_device_count_explicit_ids() {
    // Setup: ensure clean state before test
    common::test_utils::test_setup().expect("Test setup failed - please clean up before running");

    let mut manager = DeviceManager::new();
    let mut created_devices = Vec::new();

    let start_time = std::time::Instant::now();

    // Try creating devices with explicit IDs
    for id in 0..64 {
        let config = TestBlockDeviceConfig {
            dev_id: id,
            size: 64 * 1024 * 1024,
            seed: id as u64,
            fill_percent: 100,
            duplicate_percent: 0,
            random_percent: 0,
            segments: 10,
            unprivileged: true,
        };

        match manager.create(config) {
            Ok(device) => {
                if id % 10 == 0 || id < 10 {
                    log::debug!("Created device with ID {}: /dev/ublkb{}", id, device.dev_id);
                }
                assert_eq!(device.dev_id, id, "Device ID should match requested ID");
                created_devices.push(device.dev_id);
            }
            Err(e) => {
                log::error!("\nFailed to create device with ID {} with error {}", id, e);
                log::debug!(
                    "Successfully created {} devices (IDs 0-{})",
                    created_devices.len(),
                    created_devices.len() - 1
                );
                break;
            }
        }
    }

    let elapsed = start_time.elapsed();

    // Verify we created at least one device
    assert!(
        !created_devices.is_empty(),
        "Should be able to create at least one device"
    );

    log::debug!(
        "Average create time per device: {:.2?}",
        elapsed / created_devices.len() as u32
    );

    // Clean up all devices
    let cleanup_start = std::time::Instant::now();
    manager.delete_all().expect("Failed to clean up devices");
    let cleanup_elapsed = cleanup_start.elapsed();
    log::debug!(
        "Average cleanup time per device: {:.2?}\n",
        cleanup_elapsed / created_devices.len() as u32
    );

    // Teardown: verify clean state after test
    common::test_utils::test_teardown().expect("Test teardown found issues - see logs above");
}

#[test]
fn test_rapid_create_delete() {
    // Setup: ensure clean state before test
    common::test_utils::test_setup().expect("Test setup failed - please clean up before running");

    log::debug!("\n=== Testing Rapid Device Creation and Deletion ===");
    log::debug!("Creating and deleting devices 100 times...\n");

    let start_time = std::time::Instant::now();

    for i in 0..100 {
        let mut manager = DeviceManager::new();

        let config = TestBlockDeviceConfig {
            dev_id: 0,
            size: 1024 * 1024 * 1024,
            seed: i,
            fill_percent: 100,
            duplicate_percent: 0,
            random_percent: 0,
            segments: 10,
            unprivileged: true,
        };

        let device = manager
            .create(config)
            .unwrap_or_else(|_| panic!("Failed to create device {}", i));

        if i % 10 == 0 || i < 10 {
            log::debug!(
                "Iteration {}: Created and deleting /dev/ublkb{}",
                i,
                device.dev_id
            );
        }

        // Delete immediately
        manager
            .delete(device.dev_id)
            .unwrap_or_else(|_| panic!("Failed to delete device {}", i));
    }

    let elapsed = start_time.elapsed();

    log::debug!("\n=== Summary ===");
    log::debug!("Successfully completed 100 create/delete cycles");
    log::debug!("Total time: {:.2?}", elapsed);
    log::debug!("Average time per cycle: {:.2?}\n", elapsed / 100);

    // Teardown: verify clean state after test
    common::test_utils::test_teardown().expect("Test teardown found issues - see logs above");
}
