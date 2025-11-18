use clap::{Parser, Subcommand};
use parse_size::parse_size;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Error, ErrorKind};
use std::os::fd::AsFd;
use std::time::Duration;
use test_bd::{SegmentInfo, TestBlockDevice, TestBlockDeviceConfig};

#[derive(Debug, Serialize, Deserialize)]
struct DeviceInfo {
    seed: u64,
    device_id: i32,
    segments: Vec<SegmentInfo>,
}

fn write_dev_id(dev_id: i32, efd: &nix::sys::eventfd::EventFd) -> Result<i32, Error> {
    // Can't write 0 to eventfd file, otherwise the read() side may
    // not be waken up
    let id_plus_one = (dev_id as u64) + 1;
    let bytes = id_plus_one.to_le_bytes();

    nix::unistd::write(efd, &bytes)?;
    Ok(0)
}

fn read_dev_id(efd: &nix::sys::eventfd::EventFd) -> Result<i32, Error> {
    let mut buffer = [0; 8];

    let bytes_read = nix::unistd::read(efd, &mut buffer)?;
    if bytes_read == 0 {
        return Err(Error::new(ErrorKind::InvalidInput, "invalid device id"));
    }
    Ok((i64::from_le_bytes(buffer) - 1) as i32)
}

fn read_dev_id_with_timeout(
    efd: &nix::sys::eventfd::EventFd,
    timeout: Duration,
) -> Result<i32, Error> {
    use nix::poll::{PollFd, PollFlags};

    let timeout_ms = u16::try_from(timeout.as_millis()).unwrap_or(u16::MAX);
    let mut poll_fds = [PollFd::new(efd.as_fd(), PollFlags::POLLIN)];

    match nix::poll::poll(&mut poll_fds, Some(timeout_ms)) {
        Ok(n) if n > 0 => {
            // Data is available, read it
            read_dev_id(efd)
        }
        Ok(_) => {
            // Timeout
            Err(Error::new(
                ErrorKind::TimedOut,
                "Timeout waiting for device to be ready. Check log files in /tmp/test-bd_*.error",
            ))
        }
        Err(e) => Err(Error::other(format!("Poll error: {}", e))),
    }
}

fn run_daemon(config: TestBlockDeviceConfig, json_output: bool) -> Result<(), String> {
    // Validate configuration before daemonization
    config.validate()?;

    // Check if ublk control device exists
    if !std::path::Path::new("/dev/ublk-control").exists() {
        return Err("ublk control device not found at /dev/ublk-control. \
            Please ensure:\n\
            1. Kernel version is 6.0 or later (current: check 'uname -r')\n\
            2. CONFIG_BLK_DEV_UBLK is enabled in kernel config\n\
            3. ublk_drv kernel module is loaded (try 'sudo modprobe ublk_drv')"
            .to_string());
    }

    // Check if we can access the control device
    if let Err(e) = std::fs::File::open("/dev/ublk-control") {
        return Err(format!(
            "Cannot access /dev/ublk-control: {}. \
            Try running with sudo or check permissions.",
            e
        ));
    }

    let efd =
        nix::sys::eventfd::EventFd::from_value_and_flags(0, nix::sys::eventfd::EfdFlags::empty())
            .map_err(|e| format!("Failed to create eventfd: {}", e))?;

    let dev_id = config.dev_id;
    let seed = config.seed;

    // Generate segment info before daemonization if JSON output is requested
    let segments = if json_output {
        Some(config.generate_segments())
    } else {
        None
    };

    let stdout = File::create(format!("/tmp/test-bd_{}.debug", dev_id))
        .map_err(|e| format!("Failed to create debug log: {}", e))?;
    let stderr = File::create(format!("/tmp/test-bd_{}.error", dev_id))
        .map_err(|e| format!("Failed to create error log: {}", e))?;

    let daemonize = daemonize::Daemonize::new().stdout(stdout).stderr(stderr);
    match daemonize.execute() {
        daemonize::Outcome::Child(Ok(_)) => {
            // Child process - run the device with callback to notify parent
            let callback = move |actual_dev_id: i32, _segments: Vec<SegmentInfo>| {
                // Write the device ID back to the parent when device is ready
                if let Err(e) = write_dev_id(actual_dev_id, &efd) {
                    log::error!("Failed to write dev_id: {}", e);
                    eprintln!("Failed to write dev_id: {}", e);
                }
            };

            let result = TestBlockDevice::run_with_callback(config, Some(callback));

            // This will only be reached if the device stops or fails
            if let Err(e) = result {
                eprintln!("Device failed: {}", e);
                std::process::exit(1);
            }
        }
        daemonize::Outcome::Parent(Ok(_)) => {
            // Parent process - wait for device ID with timeout
            let timeout = Duration::from_secs(10); // 10 second timeout
            match read_dev_id_with_timeout(&efd, timeout) {
                Ok(id) => {
                    if json_output {
                        // Output JSON to stdout
                        if let Some(segs) = segments {
                            let device_info = DeviceInfo {
                                seed,
                                device_id: id,
                                segments: segs,
                            };
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&device_info)
                                    .map_err(|e| format!("Failed to serialize JSON: {}", e))?
                            );
                        }
                    } else {
                        // Regular output
                        println!("Device created with ID: {}", id);
                        TestBlockDevice::dump(id)
                            .map_err(|e| format!("Failed to dump device info: {}", e))?;
                    }
                }
                Err(e) => {
                    // Provide additional context for timeout errors
                    let error_msg = if e.kind() == ErrorKind::TimedOut {
                        format!(
                            "{}\n\nAdditional debugging steps:\n\
                                1. Check if daemon process is running: ps aux | grep test-bd\n\
                                2. Check error log: cat /tmp/test-bd_{}.error\n\
                                3. Check debug log: cat /tmp/test-bd_{}.debug\n\
                                4. Try running with RUST_LOG=debug for more details",
                            e, dev_id, dev_id
                        )
                    } else {
                        format!("Failed to add ublk device: {}", e)
                    };
                    return Err(error_msg);
                }
            }
        }
        _ => {
            return Err("Daemonization failed".to_string());
        }
    }

    Ok(())
}

#[derive(Parser)]
#[command(
    version,
    about,
    long_about = "Read only [RO] test block device with procedurally generated data"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Adds a block device")]
    Add {
        #[arg(long, default_value = "0")]
        id: i32,

        #[arg(short, long, default_value = "1 GiB")]
        size: Option<String>,

        #[arg(short, long, default_value = "25")]
        fill: Option<u32>,

        #[arg(short, long, default_value = "50")]
        duplicate: Option<u32>,

        #[arg(short, long, default_value = "25")]
        random: Option<u32>,

        #[arg(long, default_value = "0")]
        seed: Option<u64>,

        #[arg(long, default_value = "100")]
        segments: Option<usize>,

        #[arg(long)]
        privileged: bool,

        #[arg(short = 'J', long)]
        json: bool,
    },

    #[command(about = "Deletes a block device")]
    Del {
        #[arg(long, default_value = "0")]
        id: i32,

        #[arg(long, default_value = "false")]
        del_async: Option<bool>,
    },
}

fn main() {
    env_logger::builder()
        .format_target(false)
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::Add {
            id,
            size,
            fill,
            duplicate,
            random,
            seed,
            segments,
            privileged,
            json,
        } => {
            let fill = fill.unwrap();
            let dup = duplicate.unwrap();
            let rand = random.unwrap();
            let seed = seed.unwrap();

            if fill + dup + rand != 100 {
                eprintln!(
                    "The [fill|duplicate|random] options must total 100, current = [{},{},{}]",
                    fill, dup, rand
                );
                std::process::exit(2);
            }

            let size_bytes = parse_size(size.clone().unwrap());
            let size = match size_bytes {
                Ok(s) => {
                    if s > i64::MAX as u64 {
                        eprintln!("Error: Max size is {}", i64::MAX);
                        std::process::exit(2);
                    }
                    s
                }
                Err(e) => {
                    eprintln!("Error in size argument {:?}", e);
                    std::process::exit(2);
                }
            };

            let nseg = segments.unwrap();
            if nseg as u64 >= size / 512 {
                eprintln!(
                    "Number of segments must be less than device size / 512 {} {}",
                    nseg,
                    size / 512
                );
                std::process::exit(2);
            }

            let seed = if seed == 0 {
                let mut rng = rand::rng();
                rng.random_range(1..u64::MAX)
            } else {
                seed
            };

            if !*json {
                println!("seed = {}", seed);
            }

            let config = TestBlockDeviceConfig {
                dev_id: *id,
                size,
                seed,
                fill_percent: fill,
                duplicate_percent: dup,
                random_percent: rand,
                segments: nseg,
                unprivileged: !*privileged, // Invert: unprivileged by default
            };

            run_daemon(config, *json)
        }
        Commands::Del { id, del_async } => TestBlockDevice::delete(*id, del_async.unwrap()),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
