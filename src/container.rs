use std::fs::{remove_dir, remove_dir_all, File};
use std::io::{ErrorKind, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::fchdir;

use crate::{Error, ExecuteTask, InitTask, Pid, Process, ProcessConfig, RootFnTask, UserMapper};

#[derive(Debug, Default)]
pub struct ContainerConfig {
    pub layers: Vec<PathBuf>,
    pub hostname: String,
}

pub struct Container {
    pub(super) state_path: PathBuf,
    pub(super) cgroup_path: PathBuf,
    pub(super) user_mapper: Arc<dyn UserMapper>,
    pub(super) config: ContainerConfig,
    pub(super) pid: Option<Pid>,
}

impl Container {
    /// Starts container with initial process.
    pub fn start(&mut self, config: ProcessConfig) -> Result<Process, Error> {
        if self.pid.is_some() {
            return Err("Container already started".into());
        }
        if !self.user_mapper.is_uid_mapped(config.uid) {
            return Err(format!("User {} is not mapped", config.uid).into());
        }
        if !self.user_mapper.is_gid_mapped(config.gid) {
            return Err(format!("User {} is not mapped", config.gid).into());
        }
        let process = InitTask::start(self, config)?;
        self.pid = Some(process.pid());
        Ok(process)
    }

    /// Executes process inside container.
    pub fn execute(&self, config: ProcessConfig) -> Result<Process, Error> {
        ExecuteTask::start(self, config)
    }

    /// Kills all processes inside container.
    pub fn kill(&mut self) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .open(self.cgroup_path.join("cgroup.kill"))?;
        file.write_all("1".as_bytes())?;
        drop(file);
        self.stop()
    }

    /// Stops container.
    pub fn stop(&mut self) -> Result<(), Error> {
        let pid = match self.pid.take() {
            Some(v) => v,
            None => return Ok(()),
        };
        match waitpid(pid, Some(WaitPidFlag::__WALL)) {
            Ok(_) => Ok(()),
            Err(Errno::ECHILD) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Releases all associated resources with container.
    pub fn destroy(mut self) -> Result<(), Error> {
        let kill_err = self.kill();
        let state_err = self.remove_state();
        let cgroup_err = remove_dir(&self.cgroup_path);
        kill_err?;
        state_err?;
        Ok(cgroup_err?)
    }

    fn remove_state(&self) -> Result<(), Error> {
        let func = || Ok(remove_dir_all(&self.state_path)?);
        RootFnTask::start(self.user_mapper.as_ref(), func)
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            let _ = self.kill();
            let _ = waitpid(pid, Some(WaitPidFlag::__WALL));
        }
    }
}

pub(crate) fn ignore_kind(result: std::io::Result<()>, kind: ErrorKind) -> std::io::Result<()> {
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

pub(crate) fn new_pipe() -> Result<(File, File), Error> {
    let (rx, tx) = nix::unistd::pipe()?;
    let rx = unsafe { File::from_raw_fd(rx) };
    let tx = unsafe { File::from_raw_fd(tx) };
    Ok((rx, tx))
}

pub(crate) fn pivot_root(path: &Path) -> Result<(), Error> {
    let new_root = open(
        path,
        OFlag::O_DIRECTORY | OFlag::O_RDONLY,
        nix::sys::stat::Mode::empty(),
    )?;
    // Changes root to new path and stacks original root on the same path.
    nix::unistd::pivot_root(path, path)?;
    // Make the original root directory rslave to avoid propagating unmount event.
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        None::<&str>,
    )?;
    // Unmount the original root directory which was stacked on top of new root directory.
    umount2("/", MntFlags::MNT_DETACH)?;
    Ok(fchdir(new_root)?)
}
