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

const PROC_CGROUP: &str = "/proc/self/cgroup";
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";
const CGROUP_PROCS: &str = "cgroup.procs";

impl Cgroup {
    pub fn new(root_path: impl Into<PathBuf>, cgroup: impl AsRef<Path>) -> Result<Self, Error> {
        let root_path = root_path.into();
        let path = root_path.join(cgroup);
        Ok(Self { root_path, path })
    }

    pub fn current() -> Result<Self, Error> {
        for line in String::from_utf8(std::fs::read(PROC_CGROUP)?)?.split('\n') {
            let parts: Vec<_> = line.split(':').collect();
            if let Some(v) = parts.get(1) {
                if !v.is_empty() {
                    continue;
                }
            }
            let cgroup = parts
                .get(2)
                .ok_or("Expected cgroup path")?
                .trim_start_matches('/');
            return Cgroup::new(CGROUP_MOUNT, cgroup);
        }
        Err("Cannot resolve cgroup".into())
    }

    pub fn parent(&self) -> Option<Self> {
        let path = self.path.parent()?;
        if path.starts_with(&self.root_path) {
            let root_path = self.root_path.clone();
            let path = path.to_owned();
            Some(Self { root_path, path })
        } else {
            None
        }
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
