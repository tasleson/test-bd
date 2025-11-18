# test-bd - Test Block Device with Procedural Data Generation

[![Crates.io](https://img.shields.io/crates/v/test-bd.svg)](https://crates.io/crates/test-bd)
[![Documentation](https://docs.rs/test-bd/badge.svg)](https://docs.rs/test-bd)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

A library and CLI tool for creating deterministic, procedurally generated test block devices using ublk (userspace block device). Perfect for testing storage management tools, backup utilities, compression algorithms, and deduplication systems.

## Features

- **Procedural Data Generation**: Deterministic data patterns based on seeds - same seed always produces same data
- **Three Data Types**: Fill (zeros), Duplicate (repeating patterns), and Random (pseudo-random)
- **Configurable Size and Segments**: Create devices of any size with customizable segment distribution
- **No Storage Required**: Generate data on-the-fly without consuming disk space
- **High Performance**: Fast reads, ~3.6 GB/s on modern hardware
- **Unprivileged Mode**: Run without root when kernel supports UBLK_F_UNPRIVILEGED_DEV
- **JSON Output**: Export segment information for automated testing
- **Library + CLI**: Use as a Rust library or command-line tool

## Quick Start

### Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
test-bd = "0.1"
```

Create a test device:

```rust
use test_bd::{TestBlockDevice, TestBlockDeviceConfig};

fn main() {
    let config = TestBlockDeviceConfig {
        dev_id: -1,                  // Auto-allocate device ID
        size: 1024 * 1024 * 1024,    // 1 GiB
        seed: 12345,                 // Deterministic seed
        fill_percent: 25,            // 25% fill (zeros)
        duplicate_percent: 50,       // 50% duplicate data
        random_percent: 25,          // 25% random data
        segments: 100,               // Split into 100 segments
        unprivileged: true,          // Run without root
    };

    // Validate configuration
    config.validate().expect("Invalid configuration");

    // Run the device (blocks until interrupted)
    match TestBlockDevice::run(config) {
        Ok(dev_id) => println!("Device /dev/ublkb{} created", dev_id),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

### CLI Usage
Create a test device:

```bash
# Create a 4 GiB device with default settings
test-bd add --size "4 GiB"

# Create with custom data distribution and JSON output
test-bd add --size "10 GiB" --fill 30 --duplicate 50 --random 20 --segments 1000 -J

# Delete a device
test-bd del --id 0
```

## Use Cases

### Testing Backup Tools

Create large test devices to verify backup tools correctly handle different data types:

```rust
use test_bd::{TestBlockDevice, TestBlockDeviceConfig};

// Create a 1 TiB device for backup testing
let config = TestBlockDeviceConfig {
    dev_id: 0,
    size: 1024u64 * 1024 * 1024 * 1024,  // 1 TiB
    seed: 42,
    fill_percent: 30,      // Empty space
    duplicate_percent: 50, // Compressible data
    random_percent: 20,    // Incompressible data
    segments: 1000,
    unprivileged: true,
};

TestBlockDevice::run(config).unwrap();
// Device is now available at /dev/ublkb0
// Run: dd if=/dev/ublkb0 | your-backup-tool
```

### Testing Compression Algorithms

Verify compression ratios with known data distributions:

```bash
# Create device with 80% duplicate data (should compress well)
test-bd add --size "10 GiB" --fill 10 --duplicate 80 --random 10 --seed 999

# Test compression
dd if=/dev/ublkb0 | gzip > compressed.gz
```

### Reproducible Testing

Same seed produces identical data every time:

```bash
# Device 1 with seed 12345
test-bd add --id 1 --size "1 GiB" --seed 12345

# Device 2 with same seed - identical data
test-bd add --id 2 --size "1 GiB" --seed 12345

# Verify they're identical
$ md5sum /dev/ublkb*
959b736ca6708c62797c0a2cb911e142  /dev/ublkb1
959b736ca6708c62797c0a2cb911e142  /dev/ublkb2
```

### Using Callbacks

Get segment information when device is ready:

```rust
use test_bd::{TestBlockDevice, TestBlockDeviceConfig, Bucket};

let config = TestBlockDeviceConfig {
    dev_id: 0,
    size: 1024 * 1024 * 1024,
    seed: 42,
    fill_percent: 25,
    duplicate_percent: 50,
    random_percent: 25,
    segments: 100,
    unprivileged: true,
};

TestBlockDevice::run_with_callback(config, Some(|dev_id, segments| {
    println!("Device {} ready with {} segments", dev_id, segments.len());
    for seg in &segments {
        match seg.pattern {
            Bucket::Fill => println!("Fill: {}..{}", seg.start, seg.end),
            Bucket::Duplicate => println!("Dup: {}..{}", seg.start, seg.end),
            Bucket::Random => println!("Rand: {}..{}", seg.start, seg.end),
        }
    }
})).unwrap();
```

## Data Patterns

### Fill Pattern
- **Data**: All zeros
- **Compressibility**: Highly compressible
- **Use Case**: Testing same data pattern

### Duplicate Pattern
- **Data**: Repeating 8-byte values (0-255)
- **Compressibility**: Moderately compressible
- **Use Case**: Testing deduplication and compression

### Random Pattern
- **Data**: Pseudo-random data based on seed
- **Compressibility**: Not compressible
- **Use Case**: Testing worst-case scenarios

## Performance

- **Read Throughput**: Varies based on hardware (CPU constrained)
- **Write Support**: Read-only (writes return EINVAL)
- **Memory Usage**: Minimal - data generated on demand
- **Storage Usage**: Zero - no backing store required

```bash
# Benchmark example
$ dd if=/dev/ublkb0 of=/dev/null bs=16M
256+0 records in
256+0 records out
4294967296 bytes (4.3 GB, 4.0 GiB) copied, 1.186 s, 3.6 GB/s
```

## Requirements

- Linux kernel 6.0+ with `CONFIG_BLK_DEV_UBLK` enabled
- For unprivileged mode: `UBLK_F_UNPRIVILEGED_DEV` support in kernel
- io_uring support

Check if ublk is available:

```bash
# Check if ublk module is loaded
lsmod | grep ublk

# Load if needed
sudo modprobe ublk_drv
```

## CLI Reference

### Add Command

```bash
test-bd add [OPTIONS]

Options:
  --id <ID>                  Device ID [default: 0, -1 for auto]
  -s, --size <SIZE>          Size [default: "1 GiB"]
  -f, --fill <FILL>          Percent fill data [default: 25]
  -d, --duplicate <DUP>      Percent duplicate data [default: 50]
  -r, --random <RANDOM>      Percent random data [default: 25]
  --seed <SEED>              Seed (0 = random) [default: 0]
  --segments <SEGMENTS>      Number of segments [default: 100]
  --privileged               Run in privileged mode
  -J, --json                 Output device info as JSON
```

### Delete Command

```bash
test-bd del --id <ID>
```

## JSON Output

Export device configuration and segment information:

```bash
$ test-bd add --id 0 --size 1G --segments 3 -J --seed 12345
{
  "seed": 12345,
  "device_id": 0,
  "segments": [
    {
      "start": 0,
      "end": 32455089,
      "pattern": "Random"
    },
    {
      "start": 32455089,
      "end": 395484167,
      "pattern": "Duplicate"
    },
    {
      "start": 395484167,
      "end": 1000000000,
      "pattern": "Random"
    }
  ]
}

```

## Safety and Limitations

- **Read-Only**: Devices are read-only by design
- **Deterministic**: Same seed always produces same data
- **No Persistence**: Data exists only while device is running
- **Maximum Size**: Limited to i64::MAX bytes (~8 EiB)
- **Segments**: Must be less than device_size / 512

## Examples

See the [examples](examples/) directory:

```bash
# Run the simple example
cargo run --example simple
```

## Development

```bash
# Build
cargo build --release

# Run tests
cargo test

# Build documentation
cargo doc --open

# For local development with path dependency:
# Uncomment the libublk path dependency in Cargo.toml
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Acknowledgments

Based on the libublk-rs example ramdisk target.
