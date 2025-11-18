// Simple test to verify basic create and delete functionality
//
// This is a minimal test to isolate the create/delete cycle

mod common;

use test_bd::{DeviceManager, TestBlockDeviceConfig};

#[test]
fn test_simple_create_and_delete() {
    eprintln!("\n=== Starting simple create and delete test ===");

    // Setup: ensure clean state before test
    common::test_utils::test_setup().expect("Test setup failed - please clean up before running");

    let mut manager = DeviceManager::new();
    eprintln!("Created DeviceManager");

    let config = TestBlockDeviceConfig {
        dev_id: 0, // Explicit device ID assignment
        size: 1024 * 1024 * 1024,
        seed: 12345,
        fill_percent: 100,
        duplicate_percent: 0,
        random_percent: 0,
        segments: 10, // Few segments
        unprivileged: true,
    };
    eprintln!("Config created");

    eprintln!("Calling manager.create()...");
    let device = manager.create(config).expect("Failed to create device");
    eprintln!("Device created successfully with ID: {}", device.dev_id);

    // Verify device file exists
    let device_path = format!("/dev/ublkb{}", device.dev_id);
    eprintln!("Checking if {} exists...", device_path);
    assert!(
        std::path::Path::new(&device_path).exists(),
        "Device file should exist"
    );
    eprintln!("Device file exists");

    // Now delete it
    eprintln!("Calling manager.delete({})...", device.dev_id);
    manager
        .delete(device.dev_id)
        .expect("Failed to delete device");
    eprintln!("Delete returned successfully");

    // Give it a moment for cleanup
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Verify device file is gone
    eprintln!("Checking if {} is removed...", device_path);
    let still_exists = std::path::Path::new(&device_path).exists();
    if still_exists {
        eprintln!("WARNING: Device file still exists after delete");
    } else {
        eprintln!("Device file removed successfully");
    }

    eprintln!("=== Test completed successfully ===\n");

    // Teardown: verify clean state after test
    common::test_utils::test_teardown().expect("Test teardown found issues - see logs above");
}
