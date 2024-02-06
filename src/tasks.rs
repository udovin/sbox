use std::convert::Infallible;
use std::ffi::CString;
use std::fs::{create_dir, File};
use std::io::{ErrorKind, Read, Write};
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::exit;

use nix::mount::{mount, MsFlags};
use nix::sched::CloneFlags;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::{chdir, execvpe, fork, setgid, setgroups, sethostname, setuid, ForkResult};

use crate::{
    clone3, ignore_kind, new_pipe, pidfd_open, pivot_root, CloneArgs, CloneResult, Container,
    Error, Pid, Process, ProcessConfig, UserMapper,
};

pub(crate) struct ExecuteTask;

impl ExecuteTask {
    pub fn start(container: &Container, config: ProcessConfig) -> Result<Process, Error> {
        let pid = match container.pid {
            Some(v) => v,
            None => return Err("Container should be started".into()),
        };
        let (mut rx, tx) = new_pipe()?;
        match unsafe { fork() }? {
            ForkResult::Child => {
                drop(rx);
                if let Err(err) = Self::main(tx, container, config, pid) {
                    eprintln!("{}", err);
                }
                // Always exit with an error because this code is unreachable during normal execution.
                exit(1)
            }
            ForkResult::Parent { child } => {
                drop(tx);
                let pid = Self::read_pid(&mut rx);
                waitpid(child, Some(WaitPidFlag::__WALL))?;
                Ok(Process { pid: pid?, config })
            }
        }
    }

    fn main(
        tx: File,
        container: &Container,
        config: ProcessConfig,
        pid: Pid,
    ) -> Result<Infallible, Error> {
        let cgroup = File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        // Enter namespaces.
        let pidfd = pidfd_open(pid).map_err(|v| format!("Cannot read container pidfd: {v}"))?;
        let flags = CloneFlags::CLONE_NEWUSER
            | CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWNET
            | CloneFlags::CLONE_NEWIPC
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::from_bits_retain(nix::libc::CLONE_NEWTIME);
        nix::sched::setns(&pidfd, flags).map_err(|v| format!("Cannot enter container: {v}"))?;
        let (child_rx, child_tx) = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_parent();
        clone_args.flag_into_cgroup(&cgroup);
        match unsafe { clone3(&clone_args) }? {
            CloneResult::Child => {
                drop(cgroup);
                drop(tx);
                drop(child_rx);
                if let Err(err) = Self::child_main(child_tx, pidfd, config) {
                    eprintln!("{}", err);
                }
                // Always exit with an error because this code is unreachable during normal execution.
                exit(1)
            }
            CloneResult::Parent { child } => {
                drop(cgroup);
                drop(pidfd);
                drop(child_tx);
                if let Err(err) = Self::parent_main(child_rx, tx, child) {
                    waitpid(child, Some(WaitPidFlag::__WALL))?;
                    return Err(err);
                }
                exit(0)
            }
        }
    }

    fn child_main(mut tx: File, pidfd: File, config: ProcessConfig) -> Result<Infallible, Error> {
        // Setup cgroup namespace.
        nix::sched::setns(pidfd, CloneFlags::CLONE_NEWCGROUP)
            .map_err(|v| format!("Cannot enter container: {v}"))?;
        // Setup workdir.
        chdir(&config.work_dir)?;
        // Setup user.
        setgroups(&[])?;
        setgid(config.gid)?;
        setuid(config.uid)?;
        // Prepare exec arguments.
        let filename = CString::new(config.command[0].as_bytes())?;
        let argv: Vec<_> = config
            .command
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        let envp: Vec<_> = config
            .environ
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        // Unlock parent process.
        tx.write_all(&[0])?;
        drop(tx);
        // Run process.
        execvpe(&filename, &argv, &envp)?;
        exit(1) // Unreachable.
    }

    fn parent_main(mut rx: File, mut tx: File, pid: Pid) -> Result<(), Error> {
        // Await child process is started.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Send child pid to parent process.
        Self::write_pid(&mut tx, pid)?;
        Ok(())
    }

    fn write_pid(file: &mut File, pid: Pid) -> Result<(), Error> {
        let buf = pid.as_raw().to_le_bytes();
        file.write_all(&buf)?;
        Ok(())
    }

    fn read_pid(file: &mut File) -> Result<Pid, Error> {
        let mut buf = [0; 4];
        file.read_exact(&mut buf)?;
        Ok(Pid::from_raw(nix::libc::pid_t::from_le_bytes(buf)))
    }
}

pub(crate) struct InitTask;

impl InitTask {
    pub fn start(container: &Container, config: ProcessConfig) -> Result<Process, Error> {
        let cgroup = File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&container.cgroup_path)?;
        let (parent_rx, parent_tx) = new_pipe()?;
        let (child_rx, child_tx) = new_pipe()?;
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
                drop(cgroup);
                drop(parent_tx);
                drop(child_rx);
                if let Err(err) = Self::child_main(parent_rx, child_tx, container, config) {
                    eprintln!("{}", err);
                }
                // Always exit with an error because this code is unreachable during normal execution.
                exit(1)
            }
            CloneResult::Parent { child } => {
                drop(cgroup);
                drop(parent_rx);
                drop(child_tx);
                if let Err(err) = Self::parent_main(child_rx, parent_tx, child, container) {
                    waitpid(child, Some(WaitPidFlag::__WALL))?;
                    return Err(err);
                }
                Ok(Process { pid: child, config })
            }
        }
    }

    fn child_main(
        mut rx: File,
        mut tx: File,
        container: &Container,
        config: ProcessConfig,
    ) -> Result<Infallible, Error> {
        // Await parent process is initialized pid.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
        // Setup mount namespace.
        Self::setup_mount_namespace(container)
            .map_err(|v| format!("Cannot setup mount namespace: {v}"))?;
        // Setup uts namespace.
        Self::setup_uts_namespace(container)
            .map_err(|v| format!("Cannot setup UTS namespace: {v}"))?;
        // Setup workdir.
        chdir(&config.work_dir)?;
        // Setup user.
        setgroups(&[])?;
        setgid(config.gid)?;
        setuid(config.uid)?;
        // Prepare exec arguments.
        let filename = CString::new(config.command[0].as_bytes())?;
        let argv: Vec<_> = config
            .command
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        let envp: Vec<_> = config
            .environ
            .iter()
            .map(|v| CString::new(v.as_bytes()))
            .try_collect()?;
        // Unlock parent process.
        tx.write_all(&[0])?;
        drop(tx);
        // Run process.
        execvpe(&filename, &argv, &envp)?;
        exit(1) // Unreachable.
    }

    fn parent_main(
        mut rx: File,
        mut tx: File,
        pid: Pid,
        container: &Container,
    ) -> Result<(), Error> {
        container
            .user_mapper
            .run_map_user(pid)
            .map_err(|v| format!("Cannot setup user namespace: {v}"))?;
        // Unlock child process.
        tx.write_all(&[0])?;
        drop(tx);
        // Await child process is started.
        rx.read_exact(&mut [0; 1])?;
        drop(rx);
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
        let lowerdir = layers
            .iter()
            .map(|v| v.as_os_str().to_str())
            .try_collect::<Vec<_>>()
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
        let (parent_rx, parent_tx) = new_pipe()?;
        let (child_rx, child_tx) = new_pipe()?;
        let mut clone_args = CloneArgs::default();
        clone_args.flag_newuser();
        match unsafe { clone3(&clone_args) }? {
            CloneResult::Child => {
                drop(parent_tx);
                drop(child_rx);
                if let Err(err) = Self::child_main(parent_rx, child_tx, func) {
                    eprintln!("{}", err);
                }
                // Always exit with an error because this code is unreachable during normal execution.
                exit(1)
            }
            CloneResult::Parent { child } => {
                drop(parent_rx);
                drop(child_tx);
                let result = Self::parent_main(child_rx, parent_tx, child, user_mapper);
                // Wait for exit.
                waitpid(child, Some(WaitPidFlag::__WALL))?;
                result
            }
        }
    }

    fn child_main(rx: impl Read, tx: impl Write, func: T) -> Result<Infallible, Error> {
        // Await parent process is initialized pid.
        read_ok(rx)?;
        // Execute code inside user namespace.
        write_result(tx, func())?;
        // Exit with success.
        exit(0)
    }

    fn parent_main(
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

fn write_result(mut w: impl Write, err: Result<(), Error>) -> Result<(), Error> {
    match err {
        Ok(()) => Ok(w.write_all(&u8::to_le_bytes(0))?),
        Err(err) => {
            w.write_all(&u8::to_le_bytes(1))?;
            let msg = err.to_string();
            w.write_all(&usize::to_le_bytes(msg.as_bytes().len()))?;
            Ok(w.write_all(msg.as_bytes())?)
        }
    }
}

fn read_result(mut r: impl Read) -> Result<Result<(), Error>, Error> {
    let mut buf = [0; size_of::<u8>()];
    r.read_exact(&mut buf)?;
    match u8::from_le_bytes(buf) {
        0 => Ok(Ok(())),
        1 => {
            let mut buf = [0; size_of::<usize>()];
            r.read_exact(&mut buf)?;
            let len = usize::from_le_bytes(buf);
            let mut buf = vec![0; len];
            r.read_exact(&mut buf)?;
            Ok(Err(String::from_utf8(buf)?.into()))
        }
        _ => unreachable!(),
    }
}

fn read_ok(mut r: impl Read) -> Result<(), Error> {
    Ok(r.read_exact(&mut [0; 1])?)
}

fn write_ok(mut w: impl Write) -> Result<(), Error> {
    Ok(w.write_all(&[0])?)
}
