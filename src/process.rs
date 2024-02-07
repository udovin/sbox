use std::path::PathBuf;

use nix::sys::signal::kill;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::{Gid, Pid, Uid};

use crate::Error;

pub type Signal = nix::sys::signal::Signal;
pub type WaitStatus = nix::sys::wait::WaitStatus;

pub struct ProcessConfig {
    pub command: Vec<String>,
    pub environ: Vec<String>,
    pub work_dir: PathBuf,
    pub uid: Uid,
    pub gid: Gid,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            command: Default::default(),
            environ: Default::default(),
            work_dir: "/".into(),
            uid: Uid::from_raw(0),
            gid: Gid::from_raw(0),
        }
    }
}

pub struct Process {
    pub(crate) pid: Pid,
    #[allow(unused)]
    pub(crate) config: ProcessConfig,
}

impl Process {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn signal(&self, signal: Signal) -> Result<(), Error> {
        Ok(kill(self.pid, signal)?)
    }

    pub fn wait(&self, flags: Option<WaitPidFlag>) -> Result<WaitStatus, Error> {
        let flags = flags.unwrap_or(WaitPidFlag::empty());
        Ok(waitpid(self.pid, Some(flags | WaitPidFlag::__WALL))?)
    }
}
