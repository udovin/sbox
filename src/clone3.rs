use std::os::fd::AsRawFd;

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
        self.flags |= nix::libc::CLONE_INTO_CGROUP as u64;
        self.cgroup = cgroup.as_raw_fd() as u64;
    }
}

pub(crate) enum Clone {
    Child,
    Parent(Pid),
}

pub(crate) unsafe fn clone3(cl_args: &CloneArgs) -> Result<Clone, nix::errno::Errno> {
    let res = nix::libc::syscall(
        nix::libc::SYS_clone3,
        cl_args as *const CloneArgs,
        core::mem::size_of::<CloneArgs>(),
    );
    nix::errno::Errno::result(res).map(|v| match v {
        0 => Clone::Child,
        v => Clone::Parent(Pid::from_raw(v as nix::libc::pid_t)),
    })
}
