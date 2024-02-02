use std::ffi::CString;
use std::fs::{create_dir, File};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::mount::{mount, MsFlags};
use nix::sys::signal::kill;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::{chdir, execvpe, setgid, sethostname, setuid, Gid, Pid, Uid};

use crate::{clone3, new_pipe, pivot_root, Clone, CloneArgs, Container, Error};

pub type Signal = nix::sys::signal::Signal;
pub type WaitStatus = nix::sys::wait::WaitStatus;

pub struct ProcessConfig {
    pub command: Vec<String>,
    pub environ: Vec<String>,
    pub work_dir: PathBuf,
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
    pub uid: Uid,
    pub gid: Gid,
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
            uid: Uid::from_raw(0),
            gid: Gid::from_raw(0),
        }
    }
}

pub struct Process {
    pub(super) pid: Pid,
    pub(super) config: ProcessConfig,
}

impl Process {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn signal(&self, signal: Signal) -> Result<(), Error> {
        Ok(kill(self.pid, signal)?)
    }

    pub fn wait(&self) -> Result<WaitStatus, Error> {
        let flags = WaitPidFlag::__WALL;
        Ok(waitpid(self.pid, Some(flags))?)
    }

    pub(super) fn run_init(container: &Container, config: ProcessConfig) -> Result<Process, Error> {
        create_dir(container.state_path.join("rootfs"))?;
        create_dir(container.state_path.join("diff"))?;
        create_dir(container.state_path.join("work"))?;
        let cgroup = File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        let (parent_rx, parent_tx) = new_pipe()?;
        let (child_rx, child_tx) = new_pipe()?;
        let mut cl_args = CloneArgs::default();
        cl_args.flag_newuser();
        cl_args.flag_newns();
        cl_args.flag_newpid();
        cl_args.flag_newnet();
        cl_args.flag_newipc();
        cl_args.flag_newuts();
        cl_args.flag_newtime();
        cl_args.flag_newcgroup();
        cl_args.flag_into_cgroup(&cgroup);
        match unsafe { clone3(&cl_args) }? {
            Clone::Child => {
                drop(cgroup);
                drop(parent_tx);
                drop(child_rx);
                let process = Self {
                    pid: Pid::from_raw(0),
                    config,
                };
                process.start_child(parent_rx, child_tx, container)?;
                unreachable!()
            }
            Clone::Parent(pid) => {
                drop(cgroup);
                drop(parent_rx);
                drop(child_tx);
                let process = Self { pid, config };
                process.start_parent(child_rx, parent_tx, pid, container)?;
                Ok(process)
            }
        }
    }

    fn start_parent(
        &self,
        mut rx: File,
        mut tx: File,
        pid: Pid,
        container: &Container,
    ) -> Result<(), Error> {
        container.setup_user_namespace(pid)?;
        // Unlock child process.
        tx.write(&[0])?;
        drop(tx);
        // Await child process is started.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        Ok(())
    }

    fn start_child(&self, mut rx: File, mut tx: File, container: &Container) -> Result<(), Error> {
        // Await parent process is initialized pid.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Setup mount namespace.
        Self::setup_mount_namespace(container)?;
        Self::setup_uts_namespace(container)?;
        // Setup workdir.
        chdir(&self.config.work_dir)?;
        // Setup user.
        setuid(self.config.uid)?;
        setgid(self.config.gid)?;
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
        execvpe(&filename, &argv, &envp)?;
        unreachable!()
    }

    pub(super) fn setup_mount_namespace(container: &Container) -> Result<(), Error> {
        let rootfs = container.state_path.join("rootfs");
        let diff = container.state_path.join("diff");
        let work = container.state_path.join("work");
        // First of all make all changes are private for current root.
        mount(
            None::<&str>,
            "/".into(),
            None::<&str>,
            MsFlags::MS_SLAVE | MsFlags::MS_REC,
            None::<&str>,
        )?;
        mount(
            None::<&str>,
            "/".into(),
            None::<&str>,
            MsFlags::MS_PRIVATE,
            None::<&str>,
        )?;
        mount(
            Some(&rootfs),
            &rootfs,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )?;
        // Setup overlayfs.
        Self::setup_overlayfs(&container.config.layers, &diff, &work, &rootfs)?;
        // Setup mounts.
        Self::setup_mount(
            &rootfs,
            "sysfs",
            "/sys",
            "sysfs",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_RDONLY,
            None,
        )?;
        Self::setup_mount(
            &rootfs,
            "proc",
            "/proc",
            "proc",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None,
        )?;
        Self::setup_mount(
            &rootfs,
            "tmpfs",
            "/dev",
            "tmpfs",
            MsFlags::MS_NOSUID | MsFlags::MS_STRICTATIME,
            Some("mode=755,size=65536k"),
        )?;
        Self::setup_mount(
            &rootfs,
            "devpts",
            "/dev/pts",
            "devpts",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            Some("newinstance,ptmxmode=0666,mode=0620"),
        )?;
        Self::setup_mount(
            &rootfs,
            "tmpfs",
            "/dev/shm",
            "tmpfs",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            Some("mode=1777,size=65536k"),
        )?;
        Self::setup_mount(
            &rootfs,
            "mqueue",
            "/dev/mqueue",
            "mqueue",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None,
        )?;
        Self::setup_mount(
            &rootfs,
            "cgroup",
            "/sys/fs/cgroup",
            "cgroup2",
            MsFlags::MS_NOEXEC
                | MsFlags::MS_NOSUID
                | MsFlags::MS_NODEV
                | MsFlags::MS_RELATIME
                | MsFlags::MS_RDONLY,
            None,
        )?;
        // Pivot root.
        pivot_root(&rootfs)?;
        Ok(())
    }

    fn setup_uts_namespace(container: &Container) -> Result<(), Error> {
        Ok(sethostname(&container.config.hostname)?)
    }

    fn setup_overlayfs(
        layers: &[PathBuf],
        diff: &Path,
        work: &Path,
        rootfs: &Path,
    ) -> Result<(), Error> {
        let lowerdir = layers
            .iter()
            .map(|v| v.as_os_str().to_str())
            .try_collect::<Vec<_>>()
            .ok_or("invalid lowerdir path")?
            .join(":");
        let upperdir = diff.as_os_str().to_str().ok_or("invalid upperdir path")?;
        let workdir = work.as_os_str().to_str().ok_or("invalid workdir path")?;
        let mount_data = format!(
            "lowerdir={},upperdir={},workdir={}",
            lowerdir, upperdir, workdir,
        );
        Ok(mount(
            "overlay".into(),
            rootfs,
            "overlay".into(),
            MsFlags::empty(),
            Some(mount_data.as_str()),
        )?)
    }

    fn setup_mount(
        rootfs: &Path,
        source: &str,
        target: &str,
        fstype: &str,
        flags: MsFlags,
        data: Option<&str>,
    ) -> Result<(), Error> {
        let target = rootfs.join(target.trim_start_matches('/'));
        if let Err(err) = create_dir(&target) {
            if err.kind() != ErrorKind::AlreadyExists {
                return Err(err.into());
            }
        }
        Ok(mount(source.into(), &target, fstype.into(), flags, data)?)
    }
}
