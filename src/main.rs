///
/// Test source block device created to test things like blk-archive
///
/// Based on the libublk-rs example ramdisk target.
///
///
///
///
use clap::{Parser, Subcommand};

use data_pattern::PercentPattern;
use io_uring::IoUring;
use libublk::ctrl::UblkCtrl;
use libublk::ctrl_async::UblkCtrlAsync;
use libublk::helpers::IoBuf;
use libublk::io::{UblkDev, UblkQueue};
use libublk::uring_async::{run_uring_tasks, ublk_reap_events_with_handler, ublk_wake_task};
use libublk::{BufDesc, UblkError, UblkFlags};
use std::fs::File;
use std::io::{Error, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use parse_size::parse_size;
use rand::Rng;

mod data_pattern;

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
            let mut p = buf_addr as *mut libc::c_ulonglong;
            let full_writes: u64 = (bytes / 8) as u64;

            let mut io_gen = state.s.lock().unwrap();
            io_gen.setup(off);

            for _ in 0..full_writes {
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

fn write_dev_id(ctrl: &UblkCtrlAsync, efd: i32) -> Result<i32, Error> {
    // Can't write 0 to eventfd file, otherwise the read() side may
    // not be waken up
    let dev_id = ctrl.dev_info().dev_id as u64 + 1;
    let bytes = dev_id.to_le_bytes();

    nix::unistd::write(efd, &bytes)?;
    Ok(0)
}

fn read_dev_id(efd: i32) -> Result<i32, Error> {
    let mut buffer = [0; 8];

    let bytes_read = nix::unistd::read(efd, &mut buffer)?;
    if bytes_read == 0 {
        return Err(Error::new(ErrorKind::InvalidInput, "invalid device id"));
    }
    Ok((i64::from_le_bytes(buffer) - 1) as i32)
}

///run this ublk daemon completely in single context with
///async control command, Rust async no longer needed
fn rd_add_dev(
    dev_id: i32,
    size: u64,
    for_add: bool,
    efd: i32,
    state: &mut data_pattern::TestBdState,
    ctrl_flags: u64,
) {
    let dev_flags = if for_add {
        UblkFlags::UBLK_DEV_F_ADD_DEV
    } else {
        UblkFlags::UBLK_DEV_F_RECOVER_DEV
    };

    let _ = libublk::io::ublk_init_task_ring(|cell| {
        use std::cell::RefCell;
        if cell.get().is_none() {
            let ring = IoUring::<io_uring::squeue::Entry, io_uring::cqueue::Entry>::builder()
                .setup_cqsize(128)
                .setup_coop_taskrun()
                .build(128)
                .map_err(UblkError::IOError)?;

            cell.set(RefCell::new(ring))
                .map_err(|_| UblkError::OtherError(-libc::EEXIST))?;
        }
        Ok(())
    });

    // Create the control using the generic async task runner
    let ctrl = match create_ublk_ctrl_async(dev_id, dev_flags, ctrl_flags) {
        Ok(c) => Rc::new(c),
        Err(e) => {
            log::error!("Failed to create ublk control device {}: {}", dev_id, e);
            eprintln!("Failed to create ublk control device {}: {}", dev_id, e);
            std::process::exit(1);
        }
    };

    let tgt_init = |dev: &mut UblkDev| {
        dev.set_default_params(size);
        Ok(())
    };
    let dev_rc = match UblkDev::new_async(ctrl.get_name(), tgt_init, &ctrl) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            log::error!("Failed to create ublk device {}: {}", dev_id, e);
            eprintln!("Failed to create ublk device {}: {}", dev_id, e);
            std::process::exit(1);
        }
    };
    let dev_clone = dev_rc.clone();
    let q_rc = match UblkQueue::new(0, &dev_clone) {
        Ok(q) => Rc::new(q),
        Err(e) => {
            log::error!("Failed to create ublk queue for device {}: {}", dev_id, e);
            eprintln!("Failed to create ublk queue for device {}: {}", dev_id, e);
            std::process::exit(1);
        }
    };
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
                Err(e) => log::error!("io_task failed for tag {}: {}", tag, e),
            }
        }));
    }

    let ctrl_clone = ctrl.clone();
    let dev_clone = dev_rc.clone();
    f_vec.push(exec.spawn(async move {
        match ctrl_clone
            .configure_queue_async(&dev_clone, 0, unsafe { libc::gettid() })
            .await
        {
            Ok(r) if r >= 0 => match ctrl_clone.start_dev_async(&dev_clone).await {
                Ok(_) => {
                    if let Err(e) = write_dev_id(&ctrl_clone, efd) {
                        log::error!("Failed to write dev_id: {}", e);
                        eprintln!("Failed to write dev_id: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("Failed to start device: {}", e);
                    eprintln!("Failed to start device: {}", e);
                }
            },
            Ok(r) => {
                log::error!("configure_queue_async returned error code: {}", r);
                eprintln!("configure_queue_async returned error code: {}", r);
            }
            Err(e) => {
                log::error!("Failed to configure queue: {}", e);
                eprintln!("Failed to configure queue: {}", e);
            }
        }
    }));
    smol::block_on(exec_rc.run(async move {
        let run_ops = || while exec.try_tick() {};
        let done = || f_vec.iter().all(|task| task.is_finished());

        if let Err(e) = poll_and_handle_rings(run_ops, done, false).await {
            log::error!("poll_and_handle_rings failed: {}", e);
        }
    }));
}

fn rd_get_device_size(ctrl: &UblkCtrl) -> u64 {
    if let Ok(tgt) = ctrl.get_target_from_json() {
        tgt.dev_size
    } else {
        0
    }
}

fn test_add(
    dev_id: i32,
    size: u64,
    seed: u64,
    recover: i32,
    percents: &PercentPattern,
    segments: usize,
    unprivileged: bool,
) {
    let efd = nix::sys::eventfd::eventfd(0, nix::sys::eventfd::EfdFlags::empty()).unwrap();

    // Construct ctrl_flags based on options
    let mut ctrl_flags = libublk::sys::UBLK_F_USER_RECOVERY as u64;
    if unprivileged {
        ctrl_flags |= libublk::sys::UBLK_F_UNPRIVILEGED_DEV as u64;
    }

    // Validate and prepare everything before daemonizing so errors are visible
    let mut actual_size = size;

    if recover > 0 {
        assert!(dev_id >= 0);
        let ctrl = match UblkCtrl::new_simple(dev_id) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to open device {} for recovery: {:?}", dev_id, e);
                std::process::exit(1);
            }
        };
        actual_size = rd_get_device_size(&ctrl);

        if let Err(e) = ctrl.start_user_recover() {
            eprintln!(
                "Failed to start user recovery for device {}: {:?}",
                dev_id, e
            );
            std::process::exit(1);
        }
    }

    // Create data pattern state before daemonizing
    let m = Mutex::new(data_pattern::DataMix::create(
        actual_size,
        seed,
        segments,
        percents,
    ));
    let state = data_pattern::TestBdState { s: Rc::new(m) };

    let stdout = File::create(format!("/tmp/test-bd_{}.debug", dev_id)).unwrap();
    let stderr = File::create(format!("/tmp/test-bd_{}.error", dev_id)).unwrap();

    let daemonize = daemonize::Daemonize::new().stdout(stdout).stderr(stderr);
    match daemonize.execute() {
        daemonize::Outcome::Child(Ok(_)) => {
            let mut state = state;
            eprintln!("child, calling rd_add_dev");
            rd_add_dev(
                dev_id,
                actual_size,
                recover == 0,
                efd,
                &mut state,
                ctrl_flags,
            );
        }
        daemonize::Outcome::Parent(Ok(_)) => {
            eprintln!("parent, calling read_dev_id");
            let read_dev_id_result = read_dev_id(efd);
            eprintln!("read_dev_id returned...");
            match read_dev_id_result {
                Ok(id) => UblkCtrl::new_simple(id).unwrap().dump(),
                _ => eprintln!("Failed to add ublk device"),
            }
        }
        _ => panic!(),
    }
}

fn test_del(dev_id: i32, async_del: bool) {
    let ctrl = UblkCtrl::new_simple(dev_id);

    match ctrl {
        Ok(ctrl) => {
            if !async_del {
                if let Err(e) = ctrl.del_dev() {
                    eprintln!(
                        "Failed to remove test block device {} {:?} (sync.)",
                        dev_id, e
                    );
                }
            } else if let Err(e) = ctrl.del_dev_async() {
                eprintln!(
                    "Failed to remove test block device {} {:?} (async.)",
                    dev_id, e
                );
            }
        }
        Err(e) => {
            eprintln!("{:?}", e);
        }
    }
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
    /// Adds a block device
    Add {
        /// ID to use for new block device
        #[arg(long, default_value = "0")]
        id: i32,

        /// Size of block device, common suffixes supported ([B|M|MiB|MB|G|GiB|GB ...])
        #[arg(short, long, default_value = "1 GiB")]
        size: Option<String>,

        /// Percent fill data
        #[arg(short, long, default_value = "25")]
        fill: Option<u32>,

        /// Percent duplicate data
        #[arg(short, long, default_value = "50")]
        duplicate: Option<u32>,

        /// Percent random data
        #[arg(short, long, default_value = "25")]
        random: Option<u32>,

        /// Seed, 0 == pick one at random
        #[arg(long, default_value = "0")]
        seed: Option<u64>,

        /// Number of segment ranges the block device is broken up into (fill, dup., rand.)
        #[arg(long, default_value = "100")]
        segments: Option<usize>,

        /// Enable unprivileged mode (UBLK_F_UNPRIVILEGED_DEV)
        #[arg(short = 'p', long)]
        unprivileged: bool,
    },

    /// Recovers a block device
    Recover {
        /// ID of block device to recover
        #[arg(long)]
        id: i32,

        /// Seed used when device was created
        #[arg(long)]
        seed: u64,

        /// Percent fill data (must match original)
        #[arg(short, long, default_value = "25")]
        fill: Option<u32>,

        /// Percent duplicate data (must match original)
        #[arg(short, long, default_value = "50")]
        duplicate: Option<u32>,

        /// Percent random data (must match original)
        #[arg(short, long, default_value = "25")]
        random: Option<u32>,

        /// Number of segments (must match original)
        #[arg(long, default_value = "100")]
        segments: Option<usize>,

        /// Enable unprivileged mode (UBLK_F_UNPRIVILEGED_DEV)
        #[arg(short = 'p', long)]
        unprivileged: bool,
    },

    /// Deletes a block device
    Del {
        /// ID of block device to remove
        #[arg(long, default_value = "0")]
        id: i32,

        #[arg(long, default_value = "false")]
        del_async: Option<bool>,
    },
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Add {
            id,
            size,
            fill,
            duplicate,
            random,
            seed,
            segments,
            unprivileged,
        } => {
            let fill = fill.unwrap();
            let dup = duplicate.unwrap();
            let rand = random.unwrap();
            let seed = seed.unwrap();

            if fill + dup + rand != 100 {
                eprintln!(
                    "The [fill|duplicate|random] options must total, current = [{},{},{}] = 100",
                    fill, dup, rand
                );
                std::process::exit(2);
            }

            let percents = PercentPattern {
                fill,
                duplicates: dup,
                random: rand,
            };

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
                let mut rng = rand::thread_rng();
                rng.gen_range(1..u64::MAX)
            } else {
                seed
            };

            println!("seed = {}", seed);

            test_add(*id, size, seed, 0, &percents, nseg, *unprivileged);
        }
        Commands::Recover {
            id,
            seed,
            fill,
            duplicate,
            random,
            segments,
            unprivileged,
        } => {
            let fill = fill.unwrap();
            let dup = duplicate.unwrap();
            let rand = random.unwrap();

            if fill + dup + rand != 100 {
                eprintln!(
                    "The [fill|duplicate|random] options must total, current = [{},{},{}] = 100",
                    fill, dup, rand
                );
                std::process::exit(2);
            }

            let percents = PercentPattern {
                fill,
                duplicates: dup,
                random: rand,
            };

            let nseg = segments.unwrap();

            // For recovery, size will be read from the device
            test_add(*id, 0, *seed, 1, &percents, nseg, *unprivileged);
        }
        Commands::Del { id, del_async } => test_del(*id, del_async.unwrap()),
    }
}
