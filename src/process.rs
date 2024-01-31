use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use super::{Container, Error, IdMap};

pub type Pid = nix::unistd::Pid;
pub type Signal = nix::sys::signal::Signal;
pub type WaitStatus = nix::sys::wait::WaitStatus;

pub struct ProcessConfig {
    pub command: Vec<String>,
    pub environ: Vec<String>,
    pub work_dir: PathBuf,
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
    pub uid: u32,
    pub gid: u32,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            command: Default::default(),
            environ: Default::default(),
            work_dir: "/".into(),
            stdin: None,
            stdout: None,
            stderr: None,
            uid: 0,
            gid: 0,
        }
    }
}

pub struct Process {
    pub(super) pid: Option<Pid>,
    pub(super) config: ProcessConfig,
}

impl Process {
    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }

    pub fn signal(&self, signal: Signal) -> Result<(), Error> {
        let pid = self.pid.ok_or("process not running")?;
        Ok(nix::sys::signal::kill(pid, signal)?)
    }

    pub fn wait(&self) -> Result<WaitStatus, Error> {
        let pid = self.pid.ok_or("process not running")?;
        let flags = nix::sys::wait::WaitPidFlag::__WALL;
        Ok(nix::sys::wait::waitpid(pid, Some(flags))?)
    }

    pub(super) fn init_container(&mut self, container: &Container) -> Result<(), Error> {
        let cgroup = std::fs::File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        let (parent_rx, parent_tx) = nix::unistd::pipe()?;
        let parent_rx = unsafe { File::from_raw_fd(parent_rx) };
        let parent_tx = unsafe { File::from_raw_fd(parent_tx) };
        let (child_rx, child_tx) = nix::unistd::pipe()?;
        let child_rx = unsafe { File::from_raw_fd(child_rx) };
        let child_tx = unsafe { File::from_raw_fd(child_tx) };
        let mut clone = clone3::Clone3::default();
        clone.flag_newuser();
        clone.flag_newns();
        clone.flag_newpid();
        clone.flag_newnet();
        clone.flag_newipc();
        clone.flag_newuts();
        clone.flag_newtime();
        clone.flag_newcgroup();
        clone.flag_into_cgroup(&cgroup);
        let pid = match unsafe { clone.call() }? {
            0 => {
                drop(cgroup);
                drop(parent_tx);
                drop(child_rx);
                self.start_child(parent_rx, child_tx)?;
                unreachable!()
            }
            v => v,
        };
        drop(cgroup);
        drop(parent_rx);
        drop(child_tx);
        self.start_parent(child_rx, parent_tx, Pid::from_raw(pid), container)
    }

    fn start_parent(
        &mut self,
        mut rx: File,
        mut tx: File,
        pid: Pid,
        container: &Container,
    ) -> Result<(), Error> {
        Self::setup_user_namespace(pid, container)?;
        // Unlock child process.
        tx.write(&[0])?;
        drop(tx);
        // Await child process is started.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Save child PID.
        self.pid = Some(pid);
        Ok(())
    }

    fn start_child(&mut self, mut rx: File, mut tx: File) -> Result<(), Error> {
        // Await parent process is initialized pid.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Prepare exec arguments.
        let filename = CString::new(self.config.command[0].as_bytes())?;
        let argv: Vec<_> = self
            .config
            .command
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        let envp: Vec<_> = self
            .config
            .environ
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        // Unlock parent process.
        tx.write(&[0])?;
        drop(tx);
        // Run process.
        nix::unistd::execvpe(&filename, &argv, &envp)?;
        unreachable!()
    }

    pub(super) fn execute(&mut self, container: &Container) -> Result<(), Error> {
        todo!()
    }

    pub(super) fn setup_user_namespace(pid: Pid, container: &Container) -> Result<(), Error> {
        Self::run_newidmap(pid, "/bin/newuidmap", &container.config.uid_map)?;
        Self::run_newidmap(pid, "/bin/newgidmap", &container.config.gid_map)?;
        Ok(())
    }

    pub(super) fn run_newidmap(pid: Pid, binary: &str, id_map: &[IdMap]) -> Result<(), Error> {
        let mut cmd = std::process::Command::new(binary);
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
}
