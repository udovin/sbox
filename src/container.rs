use std::fs::{remove_dir, remove_dir_all, File};
use std::io::{ErrorKind, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::fchdir;

use crate::{Error, ExecuteTask, IdMap, InitTask, Pid, Process, ProcessConfig, RootFnTask};

pub type Uid = nix::unistd::Uid;
pub type Gid = nix::unistd::Gid;

#[derive(Debug, Default)]
pub struct ContainerConfig {
    pub layers: Vec<PathBuf>,
    pub hostname: String,
}

pub struct Container {
    pub(super) state_path: PathBuf,
    pub(super) cgroup_path: PathBuf,
    pub(super) uid_map: Vec<IdMap<Uid>>,
    pub(super) gid_map: Vec<IdMap<Gid>>,
    pub(super) config: ContainerConfig,
    pub(super) pid: Option<Pid>,
}

impl Container {
    /// Starts container with initial process.
    pub fn start(&mut self, config: ProcessConfig) -> Result<Process, Error> {
        if self.pid.is_some() {
            return Err("Container already started".into());
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
        RootFnTask::start(&self.uid_map, &self.gid_map, func)
    }

    pub(super) fn setup_user_namespace(&self, pid: Pid) -> Result<(), Error> {
        run_newidmap("/bin/newuidmap", pid, &self.uid_map)?;
        run_newidmap("/bin/newgidmap", pid, &self.gid_map)?;
        Ok(())
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

pub(crate) fn run_newidmap<T: ToString>(
    binary: &str,
    pid: Pid,
    id_map: &[IdMap<T>],
) -> Result<(), Error> {
    let mut cmd = Command::new(binary);
    cmd.arg(pid.as_raw().to_string());
    for v in id_map {
        cmd.arg(v.container_id.to_string())
            .arg(v.host_id.to_string())
            .arg(v.size.to_string());
    }
    let mut child = cmd.spawn()?;
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("{} exited with code {}", binary, status.code().unwrap_or(0)).into());
    }
    Ok(())
}
