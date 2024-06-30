use std::convert::Infallible;
use std::ffi::CString;
use std::panic::catch_unwind;
use std::path::PathBuf;

use nix::sched::CloneFlags;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::{chdir, execvpe, fork, sethostname, ForkResult, Gid, Pid, Uid};
use nix::NixPath;

use crate::{
    clone3, exit_child, new_pipe, pidfd_open, read_ok, read_pid, read_result,
    setup_mount_namespace, write_ok, write_pid, write_result, CloneArgs, CloneResult, Container,
    Error, OwnedPid,
};

pub type Signal = nix::sys::signal::Signal;
pub type WaitStatus = nix::sys::wait::WaitStatus;

#[derive(Clone, Debug, Default)]
pub struct InitProcessOptions {
    command: Vec<String>,
    environ: Vec<String>,
    work_dir: PathBuf,
    uid: Option<Uid>,
    gid: Option<Gid>,
    cgroup: PathBuf,
}

impl InitProcessOptions {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    pub fn environ(mut self, environ: Vec<String>) -> Self {
        self.environ = environ;
        self
    }

    pub fn work_dir(mut self, work_dir: PathBuf) -> Self {
        self.work_dir = work_dir;
        self
    }

    pub fn user(mut self, uid: Uid, gid: Gid) -> Self {
        self.uid = Some(uid);
        self.gid = Some(gid);
        self
    }

    pub fn cgroup(mut self, cgroup: impl Into<PathBuf>) -> Self {
        self.cgroup = cgroup.into();
        self
    }

    pub fn start(self, container: &Container) -> Result<InitProcess, Error> {
        let uid = self.uid.unwrap_or(Uid::from(0));
        if !container.user_mapper.is_uid_mapped(uid) {
            return Err(format!("User {} is not mapped", uid).into());
        }
        let gid = self.gid.unwrap_or(Gid::from(0));
        if !container.user_mapper.is_gid_mapped(gid) {
            return Err(format!("Group {} is not mapped", gid).into());
        }
        let work_dir = if !self.work_dir.is_empty() {
            self.work_dir
        } else {
            "/".into()
        };
        let command = self.command;
        let environ = self.environ;
        let cgroup = if self.cgroup.is_empty() {
            None
        } else {
            let cgroup = container.cgroup.child(self.cgroup)?;
            cgroup.create()?;
            Some(cgroup)
        };
        let cgroup_file = container.cgroup.open()?;
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
        clone_args.flag_into_cgroup(&cgroup_file);
        match unsafe { clone3(&clone_args) }
            .map_err(|v| format!("Cannot start init process: {v}"))?
        {
            CloneResult::Child => {
                let _ = catch_unwind(move || {
                    drop(cgroup_file);
                    let rx = pipe.rx();
                    let tx = child_pipe.tx();
                    exit_child(move || -> Result<Infallible, Error> {
                        // Await parent process is initialized pid.
                        read_ok(rx)?;
                        // Unlock parent process.
                        write_result(
                            tx,
                            move || -> Result<(), Error> {
                                // Setup mount namespace.
                                setup_mount_namespace(&container)
                                    .map_err(|v| format!("Cannot setup mount namespace: {v}"))?;
                                // Setup uts namespace.
                                sethostname(&container.hostname)
                                    .map_err(|v| format!("Cannot setup hostname: {v}"))?;
                                // Setup workdir.
                                chdir(&work_dir)
                                    .map_err(|v| format!("Cannot change directory: {v}"))?;
                                // Setup user.
                                container
                                    .user_mapper
                                    .set_user(uid, gid)
                                    .map_err(|v| format!("Cannot set current user: {v}"))?;
                                Ok(())
                            }(),
                        )??;
                        // Prepare exec arguments.
                        let filename = CString::new(command[0].as_bytes())?;
                        let argv = Result::<Vec<_>, _>::from_iter(
                            command.iter().map(|v| CString::new(v.as_bytes())),
                        )?;
                        let envp = Result::<Vec<_>, _>::from_iter(
                            environ.iter().map(|v| CString::new(v.as_bytes())),
                        )?;
                        // Run process.
                        Ok(execvpe(&filename, &argv, &envp)?)
                    }())
                });
                unsafe { nix::libc::_exit(2) }
            }
            CloneResult::Parent { child } => {
                let child = unsafe { OwnedPid::from_raw(child) };
                drop(cgroup_file);
                let rx = child_pipe.rx();
                let tx = pipe.tx();
                // Map user.
                container
                    .user_mapper
                    .run_map_user(child.as_raw())
                    .map_err(|v| format!("Cannot setup user namespace: {v}"))?;
                // Setup init cgroup.
                if let Some(cgroup) = cgroup {
                    cgroup
                        .add_process(child.as_raw())
                        .map_err(|v| format!("Cannot add process to cgroup: {v}"))?;
                }
                // Unlock child process.
                write_ok(tx)?;
                // Await child process result.
                read_result(rx)??;
                Ok(InitProcess::new(child.into_raw()))
            }
        }
    }
}

pub struct InitProcess {
    pid: Pid,
}

impl InitProcess {
    fn new(pid: Pid) -> Self {
        Self { pid }
    }

    pub fn as_pid(&self) -> Pid {
        self.pid
    }

    pub fn wait(&mut self) -> Result<WaitStatus, Error> {
        Ok(waitpid(self.pid, Some(WaitPidFlag::__WALL))?)
    }

    pub fn options() -> InitProcessOptions {
        InitProcessOptions::new()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessOptions {
    command: Vec<String>,
    environ: Vec<String>,
    work_dir: PathBuf,
    uid: Option<Uid>,
    gid: Option<Gid>,
    cgroup: PathBuf,
}

impl ProcessOptions {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    pub fn environ(mut self, environ: Vec<String>) -> Self {
        self.environ = environ;
        self
    }

    pub fn work_dir(mut self, work_dir: impl Into<PathBuf>) -> Self {
        self.work_dir = work_dir.into();
        self
    }

    pub fn user(mut self, uid: impl Into<Uid>, gid: impl Into<Gid>) -> Self {
        self.uid = Some(uid.into());
        self.gid = Some(gid.into());
        self
    }

    pub fn cgroup(mut self, cgroup: impl Into<PathBuf>) -> Self {
        self.cgroup = cgroup.into();
        self
    }

    pub fn start(
        self,
        container: &Container,
        init_process: &InitProcess,
    ) -> Result<Process, Error> {
        let uid = self.uid.unwrap_or(Uid::from(0));
        if !container.user_mapper.is_uid_mapped(uid) {
            return Err(format!("User {} is not mapped", uid).into());
        }
        let gid = self.gid.unwrap_or(Gid::from(0));
        if !container.user_mapper.is_gid_mapped(gid) {
            return Err(format!("Group {} is not mapped", gid).into());
        }
        let work_dir = if !self.work_dir.is_empty() {
            self.work_dir
        } else {
            "/".into()
        };
        let cgroup = if self.cgroup.is_empty() {
            None
        } else {
            let cgroup = container.cgroup.child(self.cgroup)?;
            cgroup.create()?;
            Some(cgroup)
        };
        let command = self.command;
        let environ = self.environ;
        let pid_pipe = new_pipe()?;
        match unsafe { fork() }? {
            ForkResult::Child => {
                let _ = catch_unwind(move || -> Result<(), Error> {
                    let pid_tx = pid_pipe.tx();
                    let cgroup_file = match cgroup {
                        Some(v) => v.open(),
                        None => container.cgroup.open(),
                    }?;
                    // Enter namespaces.
                    let pidfd = pidfd_open(init_process.pid)?;
                    let flags = CloneFlags::CLONE_NEWUSER
                        | CloneFlags::CLONE_NEWNS
                        | CloneFlags::CLONE_NEWPID
                        | CloneFlags::CLONE_NEWNET
                        | CloneFlags::CLONE_NEWIPC
                        | CloneFlags::CLONE_NEWUTS
                        | CloneFlags::from_bits_retain(nix::libc::CLONE_NEWTIME);
                    nix::sched::setns(&pidfd, flags)
                        .map_err(|v| format!("Cannot enter init namespaces: {v}"))?;
                    let pipe = new_pipe()?;
                    let mut clone_args = CloneArgs::default();
                    clone_args.flag_parent();
                    clone_args.flag_into_cgroup(&cgroup_file);
                    match unsafe { clone3(&clone_args) }? {
                        CloneResult::Child => {
                            let _ = catch_unwind(move || -> Result<Infallible, Error> {
                                drop(cgroup_file);
                                drop(pid_tx);
                                let tx = pipe.tx();
                                // Unlock parent process.
                                write_result(
                                    tx,
                                    move || -> Result<(), Error> {
                                        // Setup cgroup namespace.
                                        nix::sched::setns(pidfd, CloneFlags::CLONE_NEWCGROUP)
                                            .map_err(|v| {
                                                format!("Cannot enter cgroup namespace: {v}")
                                            })?;
                                        // Setup workdir.
                                        chdir(&work_dir).map_err(|v| {
                                            format!("Cannot change work directory: {v}")
                                        })?;
                                        // Setup user.
                                        container.user_mapper.set_user(uid, gid)
                                    }(),
                                )??;
                                // Prepare exec arguments.
                                let filename = CString::new(command[0].as_bytes())?;
                                let argv = Result::<Vec<_>, _>::from_iter(
                                    command.iter().map(|v| CString::new(v.as_bytes())),
                                )?;
                                let envp = Result::<Vec<_>, _>::from_iter(
                                    environ.iter().map(|v| CString::new(v.as_bytes())),
                                )?;
                                // Run process.
                                Ok(execvpe(&filename, &argv, &envp)?)
                            });
                            unsafe { nix::libc::_exit(2) }
                        }
                        CloneResult::Parent { child } => {
                            exit_child(move || -> Result<(), Error> {
                                // Send child pid to parent process.
                                write_pid(pid_tx, child)?;
                                // Await child process is started.
                                read_result(pipe.rx())?
                            }())
                        }
                    }
                });
                unsafe { nix::libc::_exit(2) }
            }
            ForkResult::Parent { child } => {
                let child = unsafe { OwnedPid::from_raw(child) };
                let rx = pid_pipe.rx();
                // Read subchild pid.
                let sibling = unsafe { OwnedPid::from_raw(read_pid(rx)?) };
                // Wait for child exit.
                child.wait_success()?;
                // Return process.
                Ok(Process::new(sibling.into_raw()))
            }
        }
    }
}

pub struct Process {
    pid: Pid,
}

impl Process {
    fn new(pid: Pid) -> Self {
        Self { pid }
    }

    pub fn as_pid(&self) -> Pid {
        self.pid
    }

    pub fn wait(&mut self) -> Result<WaitStatus, Error> {
        Ok(waitpid(self.pid, Some(WaitPidFlag::__WALL))?)
    }

    pub fn options() -> ProcessOptions {
        ProcessOptions::new()
    }
}
