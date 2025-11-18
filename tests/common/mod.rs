// Common test utilities for test-bd integration tests

#[cfg(test)]
pub mod test_utils {
    #![allow(dead_code)]

    use std::fs;
    use std::path::Path;

    /// Check if any ublk block devices exist in /dev
    /// Note: We only check for ublkb* (block devices), not ublkc* (control devices)
    /// Control devices are always present and managed by the kernel driver
    fn check_ublk_devices() -> Result<Vec<String>, String> {
        let dev_path = Path::new("/dev");

        if !dev_path.exists() {
            return Err("/dev directory does not exist".to_string());
        }

        let mut ublk_devices = Vec::new();

        match fs::read_dir(dev_path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    // Only check for ublkb* (block devices), not ublkc* (control devices)
                    if name.starts_with("ublkb") {
                        ublk_devices.push(name.to_string());
                    }
                }
            }
            Err(e) => {
                return Err(format!("Failed to read /dev directory: {}", e));
            }
        }

        Ok(ublk_devices)
    }

    /// Check if /run/ublksrvd/ directory exists and list its contents
    fn check_ublksrvd_dir() -> Result<Vec<String>, String> {
        let ublksrvd_path = Path::new("/run/ublksrvd");

        if !ublksrvd_path.exists() {
            // Directory doesn't exist, which is fine (no leftover state)
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        match fs::read_dir(ublksrvd_path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    files.push(entry.path().to_string_lossy().to_string());
                }
            }
            Err(e) => {
                return Err(format!("Failed to read /run/ublksrvd directory: {}", e));
            }
        }

        Ok(files)
    }

    /// Setup function to run before tests
    /// Ensures no ublk devices exist and /run/ublksrvd/ is empty
    pub fn test_setup() -> Result<(), String> {
        log::debug!("\n=== Test Setup ===");

        // Check for existing ublk devices
        let devices = check_ublk_devices()?;
        if !devices.is_empty() {
            let error_msg = format!(
            "Found existing ublk devices at test start: {:?}\n\
            Please clean up before running tests:\n\
            - List devices: ls -l /dev/ublk*\n\
            - Delete devices: for i in /dev/ublkb*; do test-bd del --id $(basename $i | sed 's/ublkb//'); done",
            devices
        );
            log::debug!("ERROR: {}", error_msg);
            return Err(error_msg);
        }
        log::debug!("No ublk devices found in /dev");

        // Check /run/ublksrvd/ directory
        let ublksrvd_files = check_ublksrvd_dir()?;
        if !ublksrvd_files.is_empty() {
            let error_msg = format!(
                "Found files in /run/ublksrvd/ at test start: {:?}\n\
            Please clean up before running tests:\n\
            - sudo rm -rf /run/ublksrvd/*",
                ublksrvd_files
            );
            log::debug!("ERROR: {}", error_msg);
            return Err(error_msg);
        }
        log::debug!("/run/ublksrvd/ is empty");

        log::debug!("=== Setup Complete ===\n");
        Ok(())
    }

    /// Teardown function to run after tests
    /// Verifies no ublk devices remain and /run/ublksrvd/ is empty
    /// If failures found, logs them and attempts cleanup
    pub fn test_teardown() -> Result<(), String> {
        log::debug!("\n=== Test Teardown ===");

        let mut errors = Vec::new();

        // Check for remaining ublk devices
        match check_ublk_devices() {
            Ok(devices) => {
                if !devices.is_empty() {
                    let error = format!("Found ublk devices remaining after test: {:?}", devices);
                    log::debug!("ERROR: {}", error);
                    errors.push(error);
                } else {
                    log::debug!("No ublk devices remaining");
                }
            }
            Err(e) => {
                let error = format!("Failed to check ublk devices: {}", e);
                log::debug!("ERROR: {}", error);
                errors.push(error);
            }
        }

        // Check /run/ublksrvd/ directory
        match check_ublksrvd_dir() {
            Ok(files) => {
                if !files.is_empty() {
                    let error = format!("Found files in /run/ublksrvd/ after test: {:?}", files);
                    log::debug!("ERROR: {}", error);
                    errors.push(error);

                    // Attempt to clean up /run/ublksrvd/
                    log::debug!(
                        "Attempting to cleantest_mixed_pattern_verification up /run/ublksrvd/..."
                    );
                    match fs::remove_dir_all("/run/ublksrvd") {
                        Ok(()) => {
                            log::debug!("Successfully cleaned up /run/ublksrvd/");
                            // Recreate the directory if it's expected to exist
                            if let Err(e) = fs::create_dir_all("/run/ublksrvd") {
                                log::debug!("Warning: Failed to recreate /run/ublksrvd/: {}", e);
                            }
                        }
                        Err(e) => {
                            let cleanup_error = format!("Failed to clean up /run/ublksrvd/: {}", e);
                            log::debug!("ERROR: {}", cleanup_error);
                            errors.push(cleanup_error);
                        }
                    }
                } else {
                    log::debug!("/run/ublksrvd/ is empty");
                }
            }
            Err(e) => {
                let error = format!("Failed to check /run/ublksrvd/: {}", e);
                log::debug!("ERROR: {}", error);
                errors.push(error);
            }
        }

        log::debug!("=== Teardown Complete ===\n");

        if !errors.is_empty() {
            Err(format!(
                "Teardown found {} issue(s):\n{}",
                errors.len(),
                errors.join("\n")
            ))
        } else {
            Ok(())
        }
    }

    use env_logger::{Builder, Env};
    use std::io::Write;

    static INIT: std::sync::Once = std::sync::Once::new();

    pub fn init_logging() {
        INIT.call_once(|| {
            // Set RUST_LOG to whatever you want
            let env = Env::default().default_filter_or("error");

            Builder::from_env(env)
                .format(|buf, record| {
                    let ts = buf.timestamp_millis();
                    let tid = get_tid();

                    writeln!(
                        buf,
                        "{} [tid:{}] {} - {}",
                        ts,
                        tid,
                        record.level(),
                        record.args()
                    )
                })
                //.filter_level(LevelFilter::Debug)
                .init();
        });
    }

    #[inline]
    fn get_tid() -> libc::pid_t {
        unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t }
    }

    pub fn xxd_dump(data: &[u8]) {
        use std::io::{self, Write};

        const BYTES_PER_LINE: usize = 16;
        const GROUP_SIZE: usize = 2; // xxd groups 2 bytes at a time

        let mut stdout = io::stdout().lock();
        let mut offset = 0usize;

        while offset < data.len() {
            let remaining = data.len() - offset;
            let line_len = remaining.min(BYTES_PER_LINE);
            let line_bytes = &data[offset..offset + line_len];

            // Offset
            write!(stdout, "{:08x}:", offset).unwrap();

            // Hex bytes in groups like " 4865 6c6c ..."
            let groups_per_line = BYTES_PER_LINE / GROUP_SIZE;

            for g in 0..groups_per_line {
                let base = g * GROUP_SIZE;

                // Always prefix group with a space
                write!(stdout, " ").unwrap();

                if base >= line_len {
                    // No bytes left in this group: pad "    "
                    write!(stdout, "    ").unwrap();
                    continue;
                }

                // First byte
                write!(stdout, "{:02x}", line_bytes[base]).unwrap();

                // Second byte or padding
                if base + 1 < line_len {
                    write!(stdout, "{:02x}", line_bytes[base + 1]).unwrap();
                } else {
                    write!(stdout, "  ").unwrap();
                }
            }

            // Spacer
            write!(stdout, "  ").unwrap();

            // ASCII
            for &b in line_bytes {
                let ch = if (0x20..=0x7e).contains(&b) {
                    b as char
                } else {
                    '.'
                };
                write!(stdout, "{}", ch).unwrap();
            }

            writeln!(stdout).unwrap();

            offset += line_len;
        }
    }
}
