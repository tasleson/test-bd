///
/// Test source block device created to test things like blk-archive
///
/// Based on the libublk-rs example ramdisk target.
///
use libublk::ctrl::UblkCtrl;
use libublk::helpers::IoBuf;
use libublk::io::{UblkDev, UblkQueue};
use libublk::uring_async::ublk_run_ctrl_task;
use libublk::{UblkError, UblkFlags};
use rand::{RngCore, SeedableRng};
use rand_pcg::Mcg128Xsl64;
use std::fs::File;
use std::io::{Error, ErrorKind};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

struct RandWrap {
    r: Mcg128Xsl64,
    s: u64,
}

impl RandWrap {
    fn new(seed: u64) -> Self {
        Self {
            r: Mcg128Xsl64::seed_from_u64(seed),
            s: seed,
        }
    }

    fn set_state_to(&mut self, offset: u64) {
        self.r = Mcg128Xsl64::seed_from_u64(self.s);
        self.r.advance(offset as u128);
    }

    fn next_u64(&mut self) -> u64 {
        self.r.next_u64()
    }
}

fn handle_io(q: &UblkQueue, tag: u16, buf_addr: *mut u8, state: &mut TestBdState) -> i32 {
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
                let v = io_gen.next_u64();
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

async fn io_task(q: &UblkQueue<'_>, tag: u16, state: &mut TestBdState) {
    let buf_size = q.dev.dev_info.max_io_buf_bytes as usize;
    let buffer = IoBuf::<u8>::new(buf_size);
    let addr = buffer.as_mut_ptr();
    let mut cmd_op = libublk::sys::UBLK_U_IO_FETCH_REQ;
    let mut res = 0;

    loop {
        let cmd_res = q.submit_io_cmd(tag, cmd_op, addr, res).await;
        if cmd_res == libublk::sys::UBLK_IO_RES_ABORT {
            break;
        }

        res = handle_io(q, tag, addr, state);
        cmd_op = libublk::sys::UBLK_U_IO_COMMIT_AND_FETCH_REQ;
    }
}

/// Start device in async IO task, in which both control and io rings
/// are driven in current context
fn start_dev_fn(
    exe: &smol::LocalExecutor,
    ctrl_rc: &Rc<UblkCtrl>,
    dev_arc: &Arc<UblkDev>,
    q: &UblkQueue,
) -> Result<i32, UblkError> {
    let ctrl_clone = ctrl_rc.clone();
    let dev_clone = dev_arc.clone();

    // Start device in one dedicated io task
    let task = exe.spawn(async move {
        let r = ctrl_clone.configure_queue(&dev_clone, 0, unsafe { libc::gettid() });
        if r.is_err() {
            r
        } else {
            ctrl_clone.start_dev_async(&dev_clone).await
        }
    });
    ublk_run_ctrl_task(exe, q, &task)?;
    smol::block_on(task)
}

fn write_dev_id(ctrl: &UblkCtrl, efd: i32) -> Result<i32, Error> {
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
fn rd_add_dev(dev_id: i32, size: u64, for_add: bool, efd: i32, state: &mut TestBdState) {
    let dev_flags = if for_add {
        UblkFlags::UBLK_DEV_F_ADD_DEV
    } else {
        UblkFlags::UBLK_DEV_F_RECOVER_DEV
    };
    let ctrl = Rc::new(
        libublk::ctrl::UblkCtrlBuilder::default()
            .name("test_block_device")
            .id(dev_id)
            .nr_queues(1_u16)
            .depth(128_u16)
            .dev_flags(dev_flags)
            .ctrl_flags(libublk::sys::UBLK_F_USER_RECOVERY as u64)
            .build()
            .unwrap(),
    );

    let tgt_init = |dev: &mut UblkDev| {
        dev.set_default_params(size);
        Ok(())
    };
    let dev_arc = Arc::new(UblkDev::new(ctrl.get_name(), tgt_init, &ctrl).unwrap());
    let dev_clone = dev_arc.clone();
    let q_rc = Rc::new(UblkQueue::new(0, &dev_clone).unwrap());
    let exec = smol::LocalExecutor::new();

    // spawn async io tasks
    let mut f_vec = Vec::new();

    for tag in 0..ctrl.dev_info().queue_depth {
        let q_clone = q_rc.clone();

        let mut t_c = state.clone();
        f_vec.push(exec.spawn(async move {
            io_task(&q_clone, tag, &mut t_c).await;
        }));
    }

    // start device via async task
    let res = start_dev_fn(&exec, &ctrl, &dev_arc, &q_rc);
    match res {
        Ok(_) => {
            write_dev_id(&ctrl, efd).expect("Failed to write dev_id");

            libublk::uring_async::ublk_wait_and_handle_ios(&exec, &q_rc);
        }
        _ => eprintln!("device can't be started"),
    }
    smol::block_on(async { futures::future::join_all(f_vec).await });
}

fn rd_get_device_size(ctrl: &UblkCtrl) -> u64 {
    if let Ok(tgt) = ctrl.get_target_from_json() {
        tgt.dev_size
    } else {
        0
    }
}

// Expectation is we set the offset and walk the number of blocks 8 bytes at a time
pub trait PatternGenerator {
    fn setup(&mut self, offset: u64);
    fn next_u64(&mut self) -> u64;
}

// Eventually we want an interface and different data types that implement that interface that
// each have unique behavior we are looking for.
struct TestBdState {
    s: Rc<Mutex<Box<dyn PatternGenerator>>>,
}

impl std::clone::Clone for TestBdState {
    fn clone(&self) -> Self {
        Self { s: self.s.clone() }
    }
}

struct AllRandom {
    r: RandWrap,
}

impl PatternGenerator for AllRandom {
    fn setup(&mut self, offset: u64) {
        self.r.set_state_to(offset);
    }

    fn next_u64(&mut self) -> u64 {
        self.r.next_u64()
    }
}

impl AllRandom {
    fn create(seed: u128) -> Box<dyn PatternGenerator> {
        Box::new(Self {
            r: RandWrap::new(seed as u64),
        })
    }
}

fn test_add(recover: usize) {
    let dev_id: i32 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "-1".to_string())
        .parse::<i32>()
        .unwrap();
    let s = std::env::args().nth(3).unwrap_or_else(|| "32".to_string());
    let mb = s.parse::<u64>().unwrap();
    let efd = nix::sys::eventfd::eventfd(0, nix::sys::eventfd::EfdFlags::empty()).unwrap();

    let stdout = File::create("/tmp/test-bd.debug").unwrap();
    let stderr = File::create("/tmp/test-bd.error").unwrap();

    let daemonize = daemonize::Daemonize::new().stdout(stdout).stderr(stderr);
    match daemonize.execute() {
        daemonize::Outcome::Child(Ok(_)) => {
            let mut size = mb << 20;

            if recover > 0 {
                assert!(dev_id >= 0);
                let ctrl = UblkCtrl::new_simple(dev_id).unwrap();
                size = rd_get_device_size(&ctrl);

                ctrl.start_user_recover().unwrap();
            }

            let m = Mutex::new(AllRandom::create(4534168388935060362));
            let mut state = TestBdState { s: Rc::new(m) };
            rd_add_dev(dev_id, size, recover == 0, efd, &mut state);
        }
        daemonize::Outcome::Parent(Ok(_)) => match read_dev_id(efd) {
            Ok(id) => UblkCtrl::new_simple(id).unwrap().dump(),
            _ => eprintln!("Failed to add ublk device"),
        },
        _ => panic!(),
    }
}

fn test_del(async_del: bool) {
    let s = std::env::args().nth(2).unwrap_or_else(|| "0".to_string());
    let dev_id = s.parse::<i32>().unwrap();
    let ctrl = UblkCtrl::new_simple(dev_id).unwrap();

    if !async_del {
        ctrl.del_dev().expect("fail to del_dev_async");
    } else {
        ctrl.del_dev_async().expect("fail to del_dev_async");
    }
}

fn main() {
    if let Some(cmd) = std::env::args().nth(1) {
        match cmd.as_str() {
            "add" => test_add(0),
            "recover" => test_add(1),
            "del" => test_del(false),
            "del_async" => test_del(true),
            _ => todo!(),
        }
    }
}
