use std::fs::{remove_dir, remove_dir_all, File};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::fcntl::{open, OFlag};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::fchdir;

use crate::{clone3, Clone, CloneArgs, Error, IdMap, Pid, Process, ProcessConfig};

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
            return Err("container already started".into());
        }
        let process = Process::run_init(&self, config)?;
        self.pid = Some(process.pid());
        Ok(process)
    }

    /// Executes process inside container.
    #[allow(unused)]
    pub fn execute(&self, config: ProcessConfig) -> Result<Process, Error> {
        todo!()
    }

    /// Kills all processes inside container.
    pub fn kill(&self) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .open(self.cgroup_path.join("cgroup.kill"))?;
        file.write("1".as_bytes())?;
        drop(file);
        Ok(())
    }

    /// Releases all associated resources with container.
    pub fn destroy(self) -> Result<(), Error> {
        let kill_err = self.kill();
        let state_err = self.remove_state();
        let cgroup_err = remove_dir(&self.cgroup_path);
        kill_err?;
        state_err?;
        Ok(cgroup_err?)
    }

    fn remove_state(&self) -> Result<(), Error> {
        self.remove_overlay_diff()
            .map_err(|err| format!("cannot remove container diff: {}", err))?;
        self.remove_overlay_work()
            .map_err(|err| format!("cannot remove container overlay work: {}", err))?;
        Ok(remove_dir_all(&self.state_path)
            .map_err(|err| format!("cannot remove container state: {}", err))?)
    }

    fn remove_overlay_diff(&self) -> Result<(), Error> {
        self.run_as_root(|| Ok(remove_dir_all(&self.state_path.join("diff"))?))
    }

    fn remove_overlay_work(&self) -> Result<(), Error> {
        remove_dir(&self.state_path.join("work/work"))?;
        Ok(remove_dir(&self.state_path.join("work"))?)
    }

    fn run_as_root<Fn: FnOnce() -> Result<(), Error>>(&self, func: Fn) -> Result<(), Error> {
        let (mut parent_rx, mut parent_tx) = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_newuser();
        match unsafe { clone3(&clone_args) }? {
            Clone::Child => {
                drop(parent_tx);
                // Await parent process is initialized pid.
                parent_rx.read_exact(&mut [0; 1])?;
                drop(parent_rx);
                func()?;
                std::process::exit(0)
            }
            Clone::Parent(pid) => {
                drop(parent_rx);
                // Setup user namespace.
                self.setup_user_namespace(pid)?;
                // Unlock child process.
                parent_tx.write(&[0])?;
                drop(parent_tx);
                // Wait for exit.
                waitpid(pid, Some(WaitPidFlag::__WALL))?;
            }
        };
        Ok(())
    }

    pub(super) fn setup_user_namespace(&self, pid: Pid) -> Result<(), Error> {
        run_newidmap("/bin/newuidmap", pid, &self.uid_map)?;
        run_newidmap("/bin/newgidmap", pid, &self.gid_map)?;
        Ok(())
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

fn run_newidmap<T: ToString>(binary: &str, pid: Pid, id_map: &[IdMap<T>]) -> Result<(), Error> {
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
        return Err(format!(
            "{} exited with status: {}",
            binary,
            status.code().unwrap_or(0)
        )
        .into());
    }
    Ok(())
}
