use std::convert::Infallible;
use std::ffi::CString;
use std::fs::{create_dir, File};
use std::io::{ErrorKind, Read, Write};
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::mount::{mount, MsFlags};
use nix::sched::CloneFlags;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::{chdir, execvpe, fork, sethostname, ForkResult};

use crate::{
    clone3, ignore_kind, new_pipe, pidfd_open, pivot_root, CloneArgs, CloneResult, Container,
    Error, Pid, Process, ProcessConfig, UserMapper, WaitStatus,
};

pub(crate) struct ExecuteTask;

impl ExecuteTask {
    pub fn start(container: &Container, config: ProcessConfig) -> Result<Process, Error> {
        let init_pid = match container.pid {
            Some(v) => v,
            None => return Err("Container should be started".into()),
        };
        let pipe = new_pipe()?;
        match unsafe { fork() }? {
            ForkResult::Child => {
                // std::panic::always_abort();
                exit_child(Self::run_child(pipe.tx(), container, config, init_pid))
            }
            ForkResult::Parent { child } => {
                let child = ChildGuard::new(child);
                Self::run_parent(pipe.rx(), config, child)
            }
        }
    }

    pub fn run_child(
        tx: impl Write,
        container: &Container,
        config: ProcessConfig,
        init_pid: Pid,
    ) -> Result<(), Error> {
        let cgroup = File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        // Enter namespaces.
        let pidfd = pidfd_open(init_pid)?;
        let flags = CloneFlags::CLONE_NEWUSER
            | CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWNET
            | CloneFlags::CLONE_NEWIPC
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::from_bits_retain(nix::libc::CLONE_NEWTIME);
        nix::sched::setns(&pidfd, flags)?;
        let pipe = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_parent();
        clone_args.flag_into_cgroup(&cgroup);
        match unsafe { clone3(&clone_args) }? {
            CloneResult::Child => {
                // std::panic::always_abort();
                drop(cgroup);
                drop(tx);
                exit_child(Self::run_child_child(pipe.tx(), pidfd, container, config))
            }
            CloneResult::Parent { child } => {
                drop(cgroup);
                drop(pidfd);
                Ok(Self::run_child_parent(pipe.rx(), tx, child)?)
            }
        }
    }

    fn run_parent(
        rx: impl Read,
        config: ProcessConfig,
        child: ChildGuard,
    ) -> Result<Process, Error> {
        // Read subchild pid.
        let subchild = ChildGuard::new(read_pid(rx)?);
        // Wait for child exit.
        child.wait_success()?;
        // Return process.
        Ok(Process {
            pid: subchild.into_pid(),
            config,
        })
    }

    fn run_child_child(
        tx: impl Write,
        pidfd: File,
        container: &Container,
        config: ProcessConfig,
    ) -> Result<Infallible, Error> {
        // Setup cgroup namespace.
        nix::sched::setns(pidfd, CloneFlags::CLONE_NEWCGROUP)?;
        // Setup workdir.
        chdir(&config.work_dir)?;
        // Setup user.
        container.user_mapper.set_user(config.uid, config.gid)?;
        // Prepare exec arguments.
        let filename = CString::new(config.command[0].as_bytes())?;
        let argv = Result::<Vec<_>, _>::from_iter(
            config.command.iter().map(|v| CString::new(v.as_bytes())),
        )?;
        let envp = Result::<Vec<_>, _>::from_iter(
            config.environ.iter().map(|v| CString::new(v.as_bytes())),
        )?;
        // Unlock parent process.
        write_ok(tx)?;
        // Run process.
        Ok(execvpe(&filename, &argv, &envp)?)
    }

    fn run_child_parent(rx: impl Read, tx: impl Write, pid: Pid) -> Result<(), Error> {
        // Send child pid to parent process.
        write_pid(tx, pid)?;
        // Await child process is started.
        read_ok(rx)?;
        Ok(())
    }
}

pub(crate) struct InitTask;

impl InitTask {
    pub fn start(container: &Container, config: ProcessConfig) -> Result<Process, Error> {
        let cgroup = File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        let pipe = new_pipe()?;
        let child_pipe = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_newuser();
        clone_args.flag_newns();
        clone_args.flag_newpid();
        clone_args.flag_newnet();
        clone_args.flag_newipc();
        clone_args.flag_newuts();
        clone_args.flag_newtime();
        clone_args.flag_newcgroup();
        clone_args.flag_into_cgroup(&cgroup);
        match unsafe { clone3(&clone_args) }
            .map_err(|v| format!("Cannot start init process: {v}"))?
        {
            CloneResult::Child => {
                // std::panic::always_abort();
                drop(cgroup);
                exit_child(Self::run_child(
                    pipe.rx(),
                    child_pipe.tx(),
                    container,
                    config,
                ))
            }
            CloneResult::Parent { child } => {
                let child = ChildGuard::new(child);
                drop(cgroup);
                Self::run_parent(child_pipe.rx(), pipe.tx(), child.pid(), container)?;
                Ok(Process {
                    pid: child.into_pid(),
                    config,
                })
            }
        }
    }

    fn run_child(
        rx: impl Read,
        tx: impl Write,
        container: &Container,
        config: ProcessConfig,
    ) -> Result<Infallible, Error> {
        // Await parent process is initialized pid.
        read_ok(rx)?;
        // Setup mount namespace.
        Self::setup_mount_namespace(container)
            .map_err(|v| format!("Cannot setup mount namespace: {v}"))?;
        // Setup uts namespace.
        Self::setup_uts_namespace(container)
            .map_err(|v| format!("Cannot setup UTS namespace: {v}"))?;
        // Setup workdir.
        chdir(&config.work_dir)?;
        // Setup user.
        container.user_mapper.set_user(config.uid, config.gid)?;
        // Prepare exec arguments.
        let filename = CString::new(config.command[0].as_bytes())?;
        let argv = Result::<Vec<_>, _>::from_iter(
            config.command.iter().map(|v| CString::new(v.as_bytes())),
        )?;
        let envp = Result::<Vec<_>, _>::from_iter(
            config.environ.iter().map(|v| CString::new(v.as_bytes())),
        )?;
        // Unlock parent process.
        write_ok(tx)?;
        // Run process.
        Ok(execvpe(&filename, &argv, &envp)?)
    }

    fn run_parent(
        rx: impl Read,
        tx: impl Write,
        pid: Pid,
        container: &Container,
    ) -> Result<(), Error> {
        container
            .user_mapper
            .run_map_user(pid)
            .map_err(|v| format!("Cannot setup user namespace: {v}"))?;
        // Unlock child process.
        write_ok(tx)?;
        // Await child process is started.
        read_ok(rx)?;
        Ok(())
    }

    fn setup_mount_namespace(container: &Container) -> Result<(), Error> {
        let rootfs = container.state_path.join("rootfs");
        let diff = container.state_path.join("diff");
        let work = container.state_path.join("work");
        // First of all make all changes are private for current root.
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_SLAVE | MsFlags::MS_REC,
            None::<&str>,
        )?;
        mount(
            None::<&str>,
            "/",
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
        let lowerdir = Option::<Vec<_>>::from_iter(layers.iter().map(|v| v.as_os_str().to_str()))
            .ok_or(format!("Invalid overlay lowerdir: {layers:?}"))?
            .join(":");
        let upperdir = diff
            .as_os_str()
            .to_str()
            .ok_or(format!("Invalid overlay upperdir: {diff:?}"))?;
        let workdir = work
            .as_os_str()
            .to_str()
            .ok_or(format!("Invalid overlay workdir: {work:?}"))?;
        let mount_data = format!("lowerdir={lowerdir},upperdir={upperdir},workdir={workdir}");
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
        ignore_kind(create_dir(&target), ErrorKind::AlreadyExists)?;
        Ok(mount(source.into(), &target, fstype.into(), flags, data)?)
    }
}

pub(crate) struct RootFnTask<T>(PhantomData<T>);

impl<T: FnOnce() -> Result<(), Error>> RootFnTask<T> {
    pub fn start(user_mapper: &dyn UserMapper, func: T) -> Result<(), Error> {
        let pipe = new_pipe()?;
        let child_pipe = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_newuser();
        match unsafe { clone3(&clone_args) }? {
            CloneResult::Child => {
                // std::panic::always_abort();
                exit_child(Self::run_child(pipe.rx(), child_pipe.tx(), func))
            }
            CloneResult::Parent { child } => {
                let child = ChildGuard::new(child);
                Self::run_parent(child_pipe.rx(), pipe.tx(), child.pid(), user_mapper)?;
                child.wait_success()
            }
        }
    }

    fn run_child(rx: impl Read, tx: impl Write, func: T) -> Result<(), Error> {
        // Await parent process is initialized pid.
        read_ok(rx)?;
        // Execute code inside user namespace.
        write_result(tx, func())?;
        // Successful execution.
        Ok(())
    }

    fn run_parent(
        rx: impl Read,
        tx: impl Write,
        pid: Pid,
        user_mapper: &dyn UserMapper,
    ) -> Result<(), Error> {
        // Setup user namespace.
        user_mapper.run_map_user(pid)?;
        // Unlock child process.
        write_ok(tx)?;
        // Read result from child.
        Ok(read_result(rx)?.map_err(|v| format!("Cannot run as root: {v}"))?)
    }
}

fn read_result(mut rx: impl Read) -> Result<Result<(), Error>, Error> {
    let mut buf = [0; size_of::<u8>()];
    rx.read_exact(&mut buf)?;
    match u8::from_le_bytes(buf) {
        0 => Ok(Ok(())),
        1 => {
            let mut buf = [0; size_of::<usize>()];
            rx.read_exact(&mut buf)?;
            let len = usize::from_le_bytes(buf);
            let mut buf = vec![0; len];
            rx.read_exact(&mut buf)?;
            Ok(Err(String::from_utf8(buf)?.into()))
        }
        _ => unreachable!(),
    }
}

fn write_result(mut tx: impl Write, err: Result<(), Error>) -> Result<(), Error> {
    match err {
        Ok(()) => Ok(tx.write_all(&u8::to_le_bytes(0))?),
        Err(err) => {
            tx.write_all(&u8::to_le_bytes(1))?;
            let msg = err.to_string();
            tx.write_all(&usize::to_le_bytes(msg.as_bytes().len()))?;
            Ok(tx.write_all(msg.as_bytes())?)
        }
    }
}

fn read_ok(mut rx: impl Read) -> Result<(), Error> {
    Ok(rx.read_exact(&mut [0; 1])?)
}

fn write_ok(mut tx: impl Write) -> Result<(), Error> {
    Ok(tx.write_all(&[0])?)
}

fn read_pid(mut rx: impl Read) -> Result<Pid, Error> {
    let mut buf = [0; 4];
    rx.read_exact(&mut buf)?;
    Ok(Pid::from_raw(nix::libc::pid_t::from_le_bytes(buf)))
}

fn write_pid(mut tx: impl Write, pid: Pid) -> Result<(), Error> {
    let buf = pid.as_raw().to_le_bytes();
    tx.write_all(&buf)?;
    Ok(())
}

fn exit_child<T, E>(result: Result<T, E>) -> ! {
    match result {
        Ok(_) => unsafe { nix::libc::_exit(0) },
        Err(_) => unsafe { nix::libc::_exit(1) },
    }
}

struct ChildGuard(Option<Pid>);

impl ChildGuard {
    pub fn new(pid: Pid) -> Self {
        Self(Some(pid))
    }

    pub fn pid(&self) -> Pid {
        self.0.unwrap()
    }

    pub fn into_pid(mut self) -> Pid {
        self.0.take().unwrap()
    }

    pub fn wait_success(mut self) -> Result<(), Error> {
        let status = waitpid(self.0.take().unwrap(), Some(WaitPidFlag::__WALL))?;
        match status {
            WaitStatus::Exited(_, 0) => Ok(()),
            WaitStatus::Exited(_, v) => Err(format!("Child exited with: {v}").into()),
            WaitStatus::Signaled(_, v, _) => Err(format!("Child killed with: {v}").into()),
            _ => panic!("Unexpected status: {status:?}"),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            waitpid(pid, Some(WaitPidFlag::__WALL)).unwrap();
        }
    }
}
