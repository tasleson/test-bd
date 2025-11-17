use io_uring::IoUring;
use libublk::ctrl::UblkCtrl;
use libublk::ctrl_async::UblkCtrlAsync;
use libublk::helpers::IoBuf;
use libublk::io::{UblkDev, UblkQueue};
use libublk::uring_async::{run_uring_tasks, ublk_reap_events_with_handler, ublk_wake_task};
use libublk::UblkErrorExt;
use libublk::{BufDesc, UblkError, UblkFlags};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

mod data_pattern;
pub use data_pattern::Bucket;
use data_pattern::PercentPattern;
mod position;

pub use position::IndexPos;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub start: IndexPos,
    pub end: IndexPos,
    pub pattern: Bucket,
}

impl SegmentInfo {
    pub fn count(&self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }

    pub fn size_bytes(&self) -> u64 {
        (self.end.as_u64() - self.start.as_u64()) * 8
    }
}

#[derive(Debug, Clone)]
pub struct TestBlockDeviceConfig {
    pub dev_id: i32,

    pub size: u64,

    pub seed: u64,

    pub fill_percent: u32,

    pub duplicate_percent: u32,

    pub random_percent: u32,

    pub segments: usize,

    pub unprivileged: bool,
}

impl TestBlockDeviceConfig {
    pub fn generate_segments(&self) -> Vec<SegmentInfo> {
        let percents = self.percent_pattern();
        let (_, mapping) =
            data_pattern::DataMix::create(self.size, self.seed, self.segments, &percents);

        mapping
            .into_iter()
            .map(|(range, bucket)| SegmentInfo {
                start: range.start,
                end: range.end,
                pattern: bucket,
            })
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.fill_percent + self.duplicate_percent + self.random_percent != 100 {
            return Err(format!(
                "Percentages must total 100, got: fill={}, dup={}, random={}",
                self.fill_percent, self.duplicate_percent, self.random_percent
            ));
        }

        if self.size > i64::MAX as u64 {
            return Err(format!("Size exceeds maximum: {}", i64::MAX));
        }

        if self.segments as u64 >= self.size / 512 {
            return Err(format!(
                "Number of segments ({}) must be less than device size / 512 ({})",
                self.segments,
                self.size / 512
            ));
        }

        Ok(())
    }

    pub(crate) fn percent_pattern(&self) -> PercentPattern {
        PercentPattern {
            fill: self.fill_percent,
            duplicates: self.duplicate_percent,
            random: self.random_percent,
        }
    }
}

fn handle_io(
    q: &UblkQueue,
    tag: u16,
    buf_addr: *mut u8,
    state: &mut data_pattern::TestBdState,
) -> i32 {
    let iod = q.get_iod(tag);
    let off = iod.start_sector << 9;
    let bytes = (iod.nr_sectors << 9) as i32;
    let op = iod.op_flags & 0xff;

    assert!(bytes % 8 == 0);

    match op {
        libublk::sys::UBLK_IO_OP_READ => unsafe {
            let offset_index = IndexPos::new(off / 8);
            let mut p = buf_addr as *mut libc::c_ulonglong;
            let writes: u64 = (bytes / 8) as u64;

            let mut io_gen = state.s.lock().unwrap();
            io_gen.setup(offset_index);

            for _ in 0..writes {
                let v = io_gen.next_u64().to_be();
                *p = v;
                p = p.wrapping_add(1);
            }
        },
        libublk::sys::UBLK_IO_OP_WRITE => {
            return -libc::EINVAL;
        }
        libublk::sys::UBLK_IO_OP_FLUSH => {}
        _ => {
            return -libc::EINVAL;
        }
    }

    bytes
}

async fn io_task(
    q: &UblkQueue<'_>,
    tag: u16,
    state: &mut data_pattern::TestBdState,
) -> Result<(), UblkError> {
    let buf_size = q.dev.dev_info.max_io_buf_bytes as usize;
    let buffer = IoBuf::<u8>::new(buf_size);
    let addr = buffer.as_mut_ptr();

    // Submit initial prep command - any error will exit the function
    q.submit_io_prep_cmd(tag, BufDesc::Slice(buffer.as_slice()), 0, Some(&buffer))
        .await?;

    loop {
        let res = handle_io(q, tag, addr, state);

        // Any error (including QueueIsDown) will break the loop by exiting the function
        q.submit_io_commit_cmd(tag, BufDesc::Slice(buffer.as_slice()), res)
            .await?;
    }
}

/// Poll and handle both QUEUE_RING and CTRL_URING concurrently
async fn poll_and_handle_rings<R, I>(
    run_ops: R,
    is_done: I,
    check_done: bool,
) -> Result<(), UblkError>
where
    R: Fn(),
    I: Fn() -> bool,
{
    // Helper to create async wrapper for file descriptor
    let create_async_wrapper = |fd: i32| -> Result<smol::Async<File>, UblkError> {
        let file = unsafe { File::from_raw_fd(fd) };
        smol::Async::new(file).map_err(|_| UblkError::OtherError(-libc::EINVAL))
    };

    // Get file descriptors and create async wrappers
    let queue_fd = libublk::io::with_task_io_ring(|ring| ring.as_raw_fd());
    let ctrl_fd = libublk::ctrl::with_ctrl_ring(|ring| ring.as_raw_fd());
    let async_queue = create_async_wrapper(queue_fd)?;
    let async_ctrl = create_async_wrapper(ctrl_fd)?;

    // Polling function for both rings
    let poll_both_rings = || async {
        // Submit and wait on both rings
        libublk::io::with_task_io_ring_mut(|ring| ring.submit_and_wait(0))?;
        libublk::ctrl::with_ctrl_ring_mut(|ring| ring.submit_and_wait(0))?;

        // Wait for either ring to become readable
        smol::future::race(async_queue.readable(), async_ctrl.readable())
            .await
            .map(|_| false) // No timeout
            .map_err(UblkError::IOError)
    };

    // Helper to handle events from a ring
    let handle_ring_events = |cqe: &io_uring::cqueue::Entry| {
        ublk_wake_task(cqe.user_data(), cqe);
        cqe.result() == libublk::sys::UBLK_IO_RES_ABORT
    };

    // Event reaping function for both rings
    let reap_events = |_poll_timeout| {
        let mut aborted = check_done;

        // Reap events from both rings
        let queue_result = libublk::io::with_task_io_ring_mut(|ring| {
            ublk_reap_events_with_handler(ring, |cqe| {
                if handle_ring_events(cqe) {
                    aborted = true;
                }
            })
        });

        let ctrl_result = libublk::ctrl::with_ctrl_ring_mut(|ring| {
            ublk_reap_events_with_handler(ring, |cqe| {
                if handle_ring_events(cqe) {
                    aborted = true;
                }
            })
        });

        queue_result.and(ctrl_result).map(|_| aborted)
    };

    run_uring_tasks(poll_both_rings, reap_events, run_ops, is_done).await?;

    // Prevent file descriptors from being closed when async wrappers are dropped
    let _ = async_queue.into_inner().map(|f| {
        use std::os::fd::IntoRawFd;
        f.into_raw_fd()
    });
    let _ = async_ctrl.into_inner().map(|f| {
        use std::os::fd::IntoRawFd;
        f.into_raw_fd()
    });

    Ok(())
}

/// Generic function to run ublk async uring tasks with local executor
fn ublk_uring_run_async_task<T, F, Fut>(task: Fut) -> Result<T, UblkError>
where
    F: std::future::Future<Output = Result<T, UblkError>>,
    Fut: FnOnce() -> F,
{
    let exe_rc = Rc::new(smol::LocalExecutor::new());
    let task_done = Rc::new(std::cell::RefCell::new(false));
    let task_done_clone = task_done.clone();
    let exe = exe_rc.clone();

    // Create the main task with the provided async block/closure
    let main_task = exe.spawn(async move {
        let result = task().await;
        *task_done_clone.borrow_mut() = true;
        result
    });

    // Create the event handling task
    let exe2 = exe_rc.clone();
    let event_task = exe_rc.spawn(async move {
        let run_ops = || {
            while exe2.try_tick() {}
        };
        let is_done = || *task_done.borrow();
        poll_and_handle_rings(run_ops, is_done, true).await
    });

    // Run both tasks concurrently
    smol::block_on(exe_rc.run(async {
        let (task_result, _) = futures::join!(main_task, event_task);
        task_result
    }))
}

/// Create UblkCtrl using UblkCtrlBuilder::build_async() with smol executor
fn create_ublk_ctrl_async(
    dev_id: i32,
    dev_flags: UblkFlags,
    ctrl_flags: u64,
) -> Result<UblkCtrlAsync, UblkError> {
    ublk_uring_run_async_task(|| async move {
        libublk::ctrl::UblkCtrlBuilder::default()
            .name("test_block_device")
            .id(dev_id)
            .nr_queues(1_u16)
            .depth(128_u16)
            .dev_flags(dev_flags)
            .ctrl_flags(ctrl_flags)
            .build_async()
            .await
    })
}

fn run_device<F>(
    dev_id: i32,
    size: u64,
    state: &mut data_pattern::TestBdState,
    ctrl_flags: u64,
    segments: Vec<SegmentInfo>,
    on_ready: Option<F>,
) -> Result<i32, UblkError>
where
    F: FnOnce(i32, Vec<SegmentInfo>) + 'static,
{
    log::info!(
        "run_device called: dev_id={}, size={}, ctrl_flags={:#x}, segments={}",
        dev_id,
        size,
        ctrl_flags,
        segments.len()
    );

    let dev_flags = UblkFlags::UBLK_DEV_F_ADD_DEV;

    // Initialize control ring for this thread
    libublk::ctrl::ublk_init_ctrl_task_ring(|ring_opt| {
        if ring_opt.is_none() {
            log::debug!(
                "run_device: Creating new control task ring for device {}",
                dev_id
            );
            let ring = IoUring::<io_uring::squeue::Entry128>::builder()
                .setup_cqsize(128)
                .setup_coop_taskrun()
                .build(128)
                .map_err(UblkError::IOError)
                .with_context(|| String::from("IoUring::builder error"))?;
            *ring_opt = Some(ring);
        }
        Ok(())
    })?;

    log::debug!("run_device: Initializing task ring for device {}", dev_id);
    // Initialize task ring for this thread
    libublk::io::ublk_init_task_ring(|cell| {
        use std::cell::RefCell;
        if cell.get().is_none() {
            log::debug!("run_device: Creating new task ring for device {}", dev_id);
            let ring = IoUring::<io_uring::squeue::Entry, io_uring::cqueue::Entry>::builder()
                .setup_cqsize(128)
                .setup_coop_taskrun()
                .build(128)
                .map_err(|e| {
                    log::error!(
                        "run_device: Failed to build task ring for device {}: {}. \
                        This likely indicates: \
                        (1) System limit on io_uring instances reached (check /proc/sys/kernel/io_uring/max_*), \
                        (2) Insufficient locked memory (check ulimit -l), \
                        (3) Too many open file descriptors (check ulimit -n), \
                        (4) Kernel resource exhaustion",
                        dev_id, e
                    );
                    UblkError::IOError(e)
                })?;

            cell.set(RefCell::new(ring))
                .map_err(|_| {
                    log::error!("run_device: Failed to set task ring cell for device {} (EEXIST)", dev_id);
                    UblkError::OtherError(-libc::EEXIST)
                })?;
            log::debug!("run_device: Task ring created successfully for device {}", dev_id);
        } else {
            log::debug!("run_device: Task ring already exists for device {}", dev_id);
        }
        Ok(())
    }).map_err(|e| {
        log::error!("run_device: ublk_init_task_ring failed for device {}: {:?}", dev_id, e);
        e
    })?;

    // Create the control using the generic async task runner
    log::debug!(
        "run_device: Creating ublk control (requested dev_id={})",
        dev_id
    );
    let ctrl = match create_ublk_ctrl_async(dev_id, dev_flags, ctrl_flags) {
        Ok(c) => Rc::new(c),
        Err(e) => {
            log::error!("Failed to create ublk control device {}: {}", dev_id, e);
            return Err(e);
        }
    };

    let actual_dev_id = ctrl.dev_info().dev_id;
    log::info!(
        "run_device: Actual device ID assigned: {} (requested was {})",
        actual_dev_id,
        dev_id
    );

    let tgt_init = |dev: &mut UblkDev| {
        dev.set_default_params(size);
        Ok(())
    };
    log::debug!("run_device: Creating UblkDev for device {}", actual_dev_id);
    let dev_rc = match UblkDev::new_async(ctrl.get_name(), tgt_init, &ctrl) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            log::error!("Failed to create ublk device {}: {}", actual_dev_id, e);
            return Err(e);
        }
    };
    let dev_clone = dev_rc.clone();
    log::debug!(
        "run_device: Creating UblkQueue for device {}",
        actual_dev_id
    );
    let q_rc = match UblkQueue::new(0, &dev_clone) {
        Ok(q) => Rc::new(q),
        Err(e) => {
            log::error!(
                "Failed to create ublk queue for device {}: {}",
                actual_dev_id,
                e
            );
            return Err(e);
        }
    };
    log::debug!(
        "run_device: UblkQueue created successfully for device {}",
        actual_dev_id
    );
    let exec_rc = Rc::new(smol::LocalExecutor::new());
    let exec = exec_rc.clone();

    // spawn async io tasks
    let mut f_vec = Vec::new();

    for tag in 0..ctrl.dev_info().queue_depth as u16 {
        let q_clone = q_rc.clone();

        let mut t_c = state.clone();
        f_vec.push(exec.spawn(async move {
            match io_task(&q_clone, tag, &mut t_c).await {
                Err(UblkError::QueueIsDown) | Ok(_) => {}
                Err(e) => log::warn!("io_task failed for tag {}: {}", tag, e),
            }
        }));
    }

    let ctrl_clone = ctrl.clone();
    let dev_clone = dev_rc.clone();
    let ready_callback = Rc::new(std::cell::RefCell::new(on_ready));
    let ready_callback_clone = ready_callback.clone();
    let segments_clone = segments.clone();
    f_vec.push(exec.spawn(async move {
        match ctrl_clone
            .configure_queue_async(&dev_clone, 0, unsafe { libc::gettid() })
            .await
        {
            Ok(r) if r >= 0 => match ctrl_clone.start_dev_async(&dev_clone).await {
                Ok(_) => {
                    log::info!("Device {} started successfully", actual_dev_id);
                    // Call the ready callback if provided
                    if let Some(callback) = ready_callback_clone.borrow_mut().take() {
                        callback(actual_dev_id as i32, segments_clone);
                    }
                }
                Err(e) => {
                    log::error!("Failed to start device: {}", e);
                }
            },
            Ok(r) => {
                log::error!("configure_queue_async returned error code: {}", r);
            }
            Err(e) => {
                log::error!("Failed to configure queue: {}", e);
            }
        }
    }));
    log::debug!(
        "run_device: Entering smol::block_on executor loop for device {}",
        actual_dev_id
    );
    smol::block_on(exec_rc.run(async move {
        let run_ops = || while exec.try_tick() {};
        let done = || {
            let all_finished = f_vec.iter().all(|task| task.is_finished());
            if !all_finished {
                let unfinished_count = f_vec.iter().filter(|task| !task.is_finished()).count();
                log::debug!(
                    "run_device: Waiting for {} tasks to finish (out of {} total)",
                    unfinished_count,
                    f_vec.len()
                );
            }
            all_finished
        };

        log::debug!(
            "run_device: Calling poll_and_handle_rings for device {}",
            actual_dev_id
        );
        if let Err(e) = poll_and_handle_rings(run_ops, done, false).await {
            log::error!("poll_and_handle_rings failed: {}", e);
        }
        log::debug!(
            "run_device: poll_and_handle_rings completed for device {}",
            actual_dev_id
        );
    }));

    log::debug!(
        "run_device: Exited smol::block_on for device {}, returning Ok",
        actual_dev_id
    );
    Ok(actual_dev_id as i32)
}

pub struct TestBlockDevice;

impl TestBlockDevice {
    pub fn run(config: TestBlockDeviceConfig) -> Result<i32, String> {
        Self::run_with_callback(config, None::<fn(i32, Vec<SegmentInfo>)>)
    }

    pub fn run_with_callback<F>(
        config: TestBlockDeviceConfig,
        on_ready: Option<F>,
    ) -> Result<i32, String>
    where
        F: FnOnce(i32, Vec<SegmentInfo>) + 'static,
    {
        config.validate()?;

        let percents = config.percent_pattern();
        let (pattern_gen, mapping) =
            data_pattern::DataMix::create(config.size, config.seed, config.segments, &percents);
        let m = Mutex::new(pattern_gen);
        let mut state = data_pattern::TestBdState { s: Rc::new(m) };

        // Convert mapping to SegmentInfo
        let segments: Vec<SegmentInfo> = mapping
            .into_iter()
            .map(|(range, bucket)| SegmentInfo {
                start: range.start,
                end: range.end,
                pattern: bucket,
            })
            .collect();

        let ctrl_flags = if config.unprivileged {
            libublk::sys::UBLK_F_UNPRIVILEGED_DEV as u64
        } else {
            0
        };

        run_device(
            config.dev_id,
            config.size,
            &mut state,
            ctrl_flags,
            segments,
            on_ready,
        )
        .map_err(|e| format!("Failed to run device: {}", e))
    }

    pub fn delete(dev_id: i32, _async_del: bool) -> Result<(), String> {
        log::debug!(
            "TestBlockDevice::delete ENTRY: dev_id={}, async={}",
            dev_id,
            _async_del
        );

        log::debug!(
            "TestBlockDevice::delete: Creating UblkCtrl for device {}",
            dev_id
        );
        let ctrl = UblkCtrl::new_simple(dev_id).map_err(|e| {
            let err_msg = format!(
                "Failed to open device {} for deletion: {}. \
                    This may indicate: \
                    (1) device doesn't exist, \
                    (2) insufficient permissions, \
                    (3) control ring initialization failed (possibly too many open devices/rings), \
                    (4) resource exhaustion",
                dev_id, e
            );
            log::error!("{}", err_msg);
            err_msg
        })?;
        log::debug!(
            "TestBlockDevice::delete: UblkCtrl created successfully for device {}",
            dev_id
        );

        log::debug!(
            "TestBlockDevice::delete: Calling ctrl.kill_dev() for device {}",
            dev_id
        );
        ctrl.kill_dev().map_err(|e| {
            let err_msg = format!("Failed to kill device {}: {}", dev_id, e);
            log::error!("{}", err_msg);
            err_msg
        })?;
        log::debug!(
            "TestBlockDevice::delete: ctrl.kill_dev() completed for device {}",
            dev_id
        );

        log::debug!("TestBlockDevice::delete: del_dev {dev_id}");
        ctrl.del_dev().map_err(|e| {
            let err_msg = format!("del_dev({}) failed {}", dev_id, e);
            log::error!("{}", err_msg);
            err_msg
        })?;

        log::debug!("TestBlockDevice::delete: del_dev complete {dev_id}");

        Ok(())
    }

    pub fn dump(dev_id: i32) -> Result<(), String> {
        let ctrl = UblkCtrl::new_simple(dev_id)
            .map_err(|e| format!("Failed to open device {}: {}", dev_id, e))?;
        ctrl.dump();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ManagedDevice {
    pub dev_id: i32,
    pub config: TestBlockDeviceConfig,
    pub segments: Vec<SegmentInfo>,
}

pub struct DeviceManager {
    devices: HashMap<i32, (JoinHandle<Result<i32, String>>, ManagedDevice)>,
}

type SegInfo = (i32, Vec<SegmentInfo>);

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }

    pub fn create(&mut self, config: TestBlockDeviceConfig) -> Result<ManagedDevice, String> {
        // Validate configuration first
        config.validate()?;

        // Create a channel for the device to signal when it's ready
        let (tx, rx): (Sender<SegInfo>, Receiver<SegInfo>) = mpsc::channel();

        // Clone config for use in thread
        let config_clone = config.clone();

        // Spawn a thread to run the device
        let handle = thread::Builder::new()
            .name(format!(
                "test-bd-{}",
                if config.dev_id >= 0 {
                    config.dev_id.to_string()
                } else {
                    "auto".to_string()
                }
            ))
            .spawn(move || {
                log::debug!("Device thread started for dev_id={}", config_clone.dev_id);
                let result = TestBlockDevice::run_with_callback(
                    config_clone.clone(),
                    Some(move |dev_id, segments| {
                        log::debug!("Device {} ready callback invoked", dev_id);
                        // Notify the main thread that the device is ready
                        let _ = tx.send((dev_id, segments));
                        drop(tx);
                    }),
                );
                log::debug!("Device thread: run_with_callback returned with result: {:?}", result.is_ok());
                log::debug!("Device thread: About to return from closure (this is the last line before thread exit)");
                result
            })
            .map_err(|e| format!("Failed to spawn device thread: {}", e))?;

        // Wait for the device to be ready
        let (dev_id, segments) = rx.recv().map_err(|_| {
            log::error!("Failed to receive device ready signal from thread");
            "Failed to receive device ready signal from thread. \
                The device thread may have panicked or failed to start."
                .to_string()
        })?;

        drop(rx);

        log::debug!("Received ready signal for device {}", dev_id);

        let managed_device = ManagedDevice {
            dev_id,
            config,
            segments,
        };

        // Store the thread handle and device info
        self.devices
            .insert(dev_id, (handle, managed_device.clone()));

        log::debug!(
            "Device {} successfully created and added to manager",
            dev_id
        );

        // Run udevadm settle to ensure udev has finished processing the device
        // This prevents race conditions when creating multiple devices rapidly
        log::debug!(
            "Running udevadm settle for device {} with 10 second timeout...",
            dev_id
        );
        let settle_start = std::time::Instant::now();
        match std::process::Command::new("udevadm")
            .arg("settle")
            .arg("--timeout=10")
            .output()
        {
            Ok(output) => {
                let settle_duration = settle_start.elapsed();
                if output.status.success() {
                    log::debug!(
                        "udevadm settle completed successfully for device {} in {:?}",
                        dev_id,
                        settle_duration
                    );
                } else {
                    log::warn!(
                        "udevadm settle exited with status {} for device {} after {:?}: {}",
                        output.status,
                        dev_id,
                        settle_duration,
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
            Err(e) => {
                let settle_duration = settle_start.elapsed();
                log::warn!(
                    "Failed to run udevadm settle for device {} after {:?}: {}. \
                    This may cause issues when creating multiple devices rapidly. \
                    Continuing anyway.",
                    dev_id,
                    settle_duration,
                    e
                );
            }
        }
        log::debug!(
            "Finished udevadm settle for device {}, returning from create()",
            dev_id
        );

        Ok(managed_device)
    }

    pub fn list(&self) -> Vec<ManagedDevice> {
        self.devices
            .values()
            .map(|(_, device)| device.clone())
            .collect()
    }

    pub fn delete(&mut self, dev_id: i32) -> Result<i32, String> {
        log::debug!("DeviceManager::delete called for device {}", dev_id);

        // Remove the device from our tracking
        let (handle, _) = self.devices.remove(&dev_id).ok_or_else(|| {
            let err_msg = format!(
                "Device {} is not managed by this DeviceManager. \
                    Currently managing {} devices: {:?}",
                dev_id,
                self.devices.len(),
                self.devices.keys().collect::<Vec<_>>()
            );
            log::error!("{}", err_msg);
            err_msg
        })?;

        // Delete the device (this will cause the run() call to exit)
        TestBlockDevice::delete(dev_id, false).map_err(|e| {
            let err_msg = format!(
                "Failed to delete device {}: {}. \
                    Note: Thread will still be joined if possible.",
                dev_id, e
            );
            log::warn!("{}", err_msg);
            err_msg
        })?;

        handle.join().map_err(|e| {
            let err_msg = format!("Error on join {}: {:?}", dev_id, e);
            log::warn!("{}", err_msg);
            err_msg
        })?
    }

    pub fn delete_all(&mut self) -> Result<(), String> {
        let dev_ids: Vec<i32> = self.devices.keys().copied().collect();
        log::info!(
            "DeviceManager::delete_all called for {} devices: {:?}",
            dev_ids.len(),
            dev_ids
        );

        let mut first_error = None;
        let mut deleted_count = 0;
        let mut failed_count = 0;

        for (index, dev_id) in dev_ids.iter().enumerate() {
            log::debug!(
                "Deleting device {}/{}: dev_id={}",
                index + 1,
                dev_ids.len(),
                dev_id
            );

            match self.delete(*dev_id) {
                Ok(rc) => {
                    deleted_count += 1;
                    log::debug!(
                        "Successfully deleted device {} ({}/{}) with return code {}",
                        dev_id,
                        deleted_count,
                        dev_ids.len(),
                        rc
                    );
                }
                Err(e) => {
                    failed_count += 1;
                    log::error!(
                        "Failed to delete device {} ({}/{}): {}",
                        dev_id,
                        index + 1,
                        dev_ids.len(),
                        e
                    );
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        log::info!(
            "DeviceManager::delete_all completed: {} succeeded, {} failed out of {} total",
            deleted_count,
            failed_count,
            dev_ids.len()
        );

        if let Some(e) = first_error {
            Err(format!(
                "Failed to delete all devices: {} succeeded, {} failed. First error: {}",
                deleted_count, failed_count, e
            ))
        } else {
            Ok(())
        }
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DeviceManager {
    fn drop(&mut self) {
        let _ = self.delete_all();
    }
}
