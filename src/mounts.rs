use nix::fcntl::{open, OFlag};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::unistd::fchdir;
use std::fmt::Debug;
use std::fs::create_dir;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::{ignore_kind, Container, Error};

pub trait Mount: Send + Sync + Debug {
    fn mount(&self, rootfs: &Path) -> Result<(), Error>;
}

#[derive(Debug, Clone)]
pub struct OverlayMount {
    pub lowerdir: Vec<PathBuf>,
    pub upperdir: PathBuf,
    pub workdir: PathBuf,
}

impl OverlayMount {
    pub fn new(lowerdir: Vec<PathBuf>, upperdir: PathBuf, workdir: PathBuf) -> Self {
        Self {
            lowerdir,
            upperdir,
            workdir,
        }
    }
}

impl Mount for OverlayMount {
    fn mount(&self, rootfs: &Path) -> Result<(), Error> {
        let lowerdir =
            Option::<Vec<_>>::from_iter(self.lowerdir.iter().map(|v| v.as_os_str().to_str()))
                .ok_or(format!("Invalid overlay lowerdir: {:?}", self.lowerdir))?
                .join(":");
        let upperdir = self
            .upperdir
            .as_os_str()
            .to_str()
            .ok_or(format!("Invalid overlay upperdir: {:?}", self.upperdir))?;
        let workdir = self
            .workdir
            .as_os_str()
            .to_str()
            .ok_or(format!("Invalid overlay workdir: {:?}", self.workdir))?;
        let mount_data = format!("lowerdir={lowerdir},upperdir={upperdir},workdir={workdir}");
        Ok(mount(
            "overlay".into(),
            rootfs,
            "overlay".into(),
            MsFlags::empty(),
            Some(mount_data.as_str()),
        )?)
    }
}

#[derive(Debug, Clone)]
pub struct BaseMounts {}

impl BaseMounts {
    pub fn new() -> Self {
        Self {}
    }
}

impl Mount for BaseMounts {
    fn mount(&self, rootfs: &Path) -> Result<(), Error> {
        setup_mount(
            &rootfs,
            "sysfs",
            "/sys",
            "sysfs",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_RDONLY,
            None,
        )?;
        setup_mount(
            &rootfs,
            "proc",
            "/proc",
            "proc",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None,
        )?;
        setup_mount(
            &rootfs,
            "tmpfs",
            "/dev",
            "tmpfs",
            MsFlags::MS_NOSUID | MsFlags::MS_STRICTATIME,
            Some("mode=755,size=65536k"),
        )?;
        setup_mount(
            &rootfs,
            "devpts",
            "/dev/pts",
            "devpts",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            Some("newinstance,ptmxmode=0666,mode=0620"),
        )?;
        setup_mount(
            &rootfs,
            "tmpfs",
            "/dev/shm",
            "tmpfs",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            Some("mode=1777,size=65536k"),
        )?;
        setup_mount(
            &rootfs,
            "mqueue",
            "/dev/mqueue",
            "mqueue",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None,
        )?;
        setup_mount(
            &rootfs,
            "cgroup",
            "/sys/fs/cgroup",
            "cgroup2",
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_RELATIME,
            None,
        )?;
        Ok(())
    }
}

pub(crate) fn setup_mount_namespace(container: &Container) -> Result<(), Error> {
    // First of all make all changes are private for current root.
    remount_private_root(&container.rootfs)?;
    // Setup mounts.
    for mount in &container.mounts {
        mount.mount(&container.rootfs)?;
    }
    // Pivot root.
    pivot_root(&container.rootfs)
}

fn remount_private_root(path: &Path) -> Result<(), Error> {
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
    Ok(mount(
        Some(path),
        path,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )?)
}

fn pivot_root(path: &Path) -> Result<(), Error> {
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
