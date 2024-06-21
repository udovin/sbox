use std::fs::{create_dir_all, remove_dir, File};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::{Error, Pid};

#[derive(Clone, Debug)]
pub struct Cgroup {
    root_path: PathBuf,
    path: PathBuf,
}

const CGROUP_PROCS: &str = "cgroup.procs";

impl Cgroup {
    pub fn new(root_path: impl Into<PathBuf>, cgroup: impl AsRef<Path>) -> Result<Self, Error> {
        let root_path = root_path.into();
        let path = root_path.join(cgroup);
        Ok(Self { root_path, path })
    }

    pub fn child(&self, cgroup: impl AsRef<Path>) -> Result<Self, Error> {
        let root_path = self.root_path.clone();
        let path = self.path.join(cgroup);
        Ok(Self { root_path, path })
    }

    pub fn create(&self) -> Result<(), Error> {
        Ok(create_dir_all(&self.path)?)
    }

    pub fn remove(&self) -> Result<(), Error> {
        Ok(remove_dir(&self.path)?)
    }

    pub fn add_process(&self, pid: Pid) -> Result<(), Error> {
        Ok(File::options()
            .create(false)
            .write(true)
            .truncate(false)
            .open(self.path.join(CGROUP_PROCS))?
            .write_all(pid.to_string().as_bytes())?)
    }

    pub fn open(&self) -> Result<File, Error> {
        Ok(File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&self.path)?)
    }
}
