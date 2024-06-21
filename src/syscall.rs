use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::{errno::Errno, libc::syscall};

use crate::Error;

pub type Pid = nix::unistd::Pid;

#[repr(C, align(8))]
#[derive(Debug, Default)]
pub(crate) struct CloneArgs {
    pub flags: u64,
    pub pidfd: u64,
    pub child_tid: u64,
    pub parent_tid: u64,
    pub exit_signal: u64,
    pub stack: u64,
    pub stack_size: u64,
    pub tls: u64,
    pub set_tid: u64,
    pub set_tid_size: u64,
    pub cgroup: u64,
}

impl CloneArgs {
    pub fn flag_parent(&mut self) {
        self.flags |= nix::libc::CLONE_PARENT as u64;
    }

    pub fn flag_newuser(&mut self) {
        self.flags |= nix::libc::CLONE_NEWUSER as u64;
    }

    pub fn flag_newns(&mut self) {
        self.flags |= nix::libc::CLONE_NEWNS as u64;
    }

    pub fn flag_newpid(&mut self) {
        self.flags |= nix::libc::CLONE_NEWPID as u64;
    }

    pub fn flag_newnet(&mut self) {
        self.flags |= nix::libc::CLONE_NEWNET as u64;
    }

    pub fn flag_newipc(&mut self) {
        self.flags |= nix::libc::CLONE_NEWIPC as u64;
    }

    pub fn flag_newuts(&mut self) {
        self.flags |= nix::libc::CLONE_NEWUTS as u64;
    }

    pub fn flag_newtime(&mut self) {
        self.flags |= nix::libc::CLONE_NEWTIME as u64;
    }

    pub fn flag_newcgroup(&mut self) {
        self.flags |= nix::libc::CLONE_NEWCGROUP as u64;
    }

    pub fn flag_into_cgroup<T: AsRawFd>(&mut self, cgroup: &T) {
        // self.flags |= nix::libc::CLONE_INTO_CGROUP as u64;
        self.flags |= 0x200000000;
        self.cgroup = cgroup.as_raw_fd() as u64;
    }
}

pub(crate) enum CloneResult {
    Child,
    Parent { child: Pid },
}

pub(crate) unsafe fn clone3(cl_args: &CloneArgs) -> Result<CloneResult, Errno> {
    let res = syscall(
        nix::libc::SYS_clone3,
        cl_args as *const CloneArgs,
        core::mem::size_of::<CloneArgs>(),
    );
    Errno::result(res).map(|v| match v {
        0 => CloneResult::Child,
        v => CloneResult::Parent {
            child: Pid::from_raw(v as nix::libc::pid_t),
        },
    })
}

pub(crate) fn pidfd_open(pid: Pid) -> Result<File, Errno> {
    let res = unsafe {
        syscall(
            nix::libc::SYS_pidfd_open,
            pid.as_raw(),
            0 as nix::libc::c_uint,
        )
    };
    Errno::result(res).map(|v| unsafe { File::from_raw_fd(v as RawFd) })
}

pub(crate) struct Pipe {
    rx: File,
    tx: File,
}

impl Pipe {
    pub fn rx(self) -> impl Read {
        drop(self.tx);
        self.rx
    }

    pub fn tx(self) -> impl Write {
        drop(self.rx);
        self.tx
    }
}

pub(crate) fn new_pipe() -> Result<Pipe, Error> {
    let (rx, tx) = nix::unistd::pipe()?;
    let rx = unsafe { File::from_raw_fd(rx) };
    let tx = unsafe { File::from_raw_fd(tx) };
    Ok(Pipe { rx, tx })
}

pub(super) fn read_result(mut rx: impl Read) -> Result<Result<(), Error>, Error> {
    let mut buf = [0; std::mem::size_of::<u8>()];
    rx.read_exact(&mut buf)?;
    match u8::from_le_bytes(buf) {
        0 => Ok(Ok(())),
        1 => {
            let mut buf = [0; std::mem::size_of::<usize>()];
            rx.read_exact(&mut buf)?;
            let len = usize::from_le_bytes(buf);
            let mut buf = vec![0; len];
            rx.read_exact(&mut buf)?;
            Ok(Err(String::from_utf8(buf)?.into()))
        }
        _ => unreachable!(),
    }
}

pub(super) fn write_result(mut tx: impl Write, result: Result<(), Error>) -> Result<(), Error> {
    match result {
        Ok(()) => Ok(tx.write_all(&u8::to_le_bytes(0))?),
        Err(err) => {
            tx.write_all(&u8::to_le_bytes(1))?;
            let msg = err.to_string();
            tx.write_all(&usize::to_le_bytes(msg.as_bytes().len()))?;
            Ok(tx.write_all(msg.as_bytes())?)
        }
    }
}

pub(super) fn read_ok(mut rx: impl Read) -> Result<(), Error> {
    Ok(rx.read_exact(&mut [0; 1])?)
}

pub(super) fn write_ok(mut tx: impl Write) -> Result<(), Error> {
    Ok(tx.write_all(&[0])?)
}

pub(super) fn read_pid(mut rx: impl Read) -> Result<Pid, Error> {
    let mut buf = [0; 4];
    rx.read_exact(&mut buf)?;
    Ok(Pid::from_raw(nix::libc::pid_t::from_le_bytes(buf)))
}

pub(super) fn write_pid(mut tx: impl Write, pid: Pid) -> Result<(), Error> {
    let buf = pid.as_raw().to_le_bytes();
    tx.write_all(&buf)?;
    Ok(())
}

pub(super) fn exit_child<T, E>(result: Result<T, E>) -> ! {
    match result {
        Ok(_) => unsafe { nix::libc::_exit(0) },
        Err(_) => unsafe { nix::libc::_exit(1) },
    }
}

pub(super) struct OwnedPid(Option<Pid>);

impl OwnedPid {
    pub unsafe fn from_raw(pid: Pid) -> Self {
        Self(Some(pid))
    }

    pub fn as_raw(&self) -> Pid {
        self.0.unwrap()
    }

    pub fn into_raw(mut self) -> Pid {
        self.0.take().unwrap()
    }

    pub fn wait_success(self) -> Result<(), Error> {
        let status = waitpid(self.into_raw(), Some(WaitPidFlag::__WALL))?;
        match status {
            WaitStatus::Exited(_, 0) => Ok(()),
            WaitStatus::Exited(_, v) => Err(format!("Child exited with: {v}").into()),
            WaitStatus::Signaled(_, v, _) => Err(format!("Child killed with: {v}").into()),
            _ => panic!("Unexpected status: {status:?}"),
        }
    }
}

impl Drop for OwnedPid {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            waitpid(pid, Some(WaitPidFlag::__WALL)).unwrap();
        }
    }
}

pub(crate) fn ignore_kind(
    result: std::io::Result<()>,
    kind: std::io::ErrorKind,
) -> std::io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            if err.kind() == kind {
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}
