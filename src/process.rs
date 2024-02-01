use std::ffi::CString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::mount::{mount, umount2, MntFlags, MsFlags};

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
        std::fs::create_dir(container.state_path.join("rootfs"))?;
        std::fs::create_dir(container.state_path.join("diff"))?;
        std::fs::create_dir(container.state_path.join("work"))?;
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
                self.start_child(parent_rx, child_tx, container)?;
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

    fn start_child(
        &mut self,
        mut rx: File,
        mut tx: File,
        container: &Container,
    ) -> Result<(), Error> {
        // Await parent process is initialized pid.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Setup mount namespace.
        Self::setup_mount_namespace(container)?;
        Self::setup_uts_namespace(container)?;
        // Setup workdir.
        nix::unistd::chdir(&self.config.work_dir)?;
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

    fn setup_user_namespace(pid: Pid, container: &Container) -> Result<(), Error> {
        Self::run_newidmap(pid, "/bin/newuidmap", &container.config.uid_map)?;
        Self::run_newidmap(pid, "/bin/newgidmap", &container.config.gid_map)?;
        Ok(())
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
        Self::pivot_root(&rootfs)?;
        Ok(())
    }

    fn setup_uts_namespace(container: &Container) -> Result<(), Error> {
        Ok(nix::unistd::sethostname("sbox")?)
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
            nix::mount::MsFlags::empty(),
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
        if let Err(err) = std::fs::create_dir(&target) {
            if err.kind() != ErrorKind::AlreadyExists {
                return Err(err.into());
            }
        }
        Ok(mount(source.into(), &target, fstype.into(), flags, data)?)
    }

    fn pivot_root(path: &Path) -> Result<(), Error> {
        let new_root = nix::fcntl::open(
            path,
            nix::fcntl::OFlag::O_DIRECTORY | nix::fcntl::OFlag::O_RDONLY,
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
        Ok(nix::unistd::fchdir(new_root)?)
    }

    fn run_newidmap(pid: Pid, binary: &str, id_map: &[IdMap]) -> Result<(), Error> {
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
