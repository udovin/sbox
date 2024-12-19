use std::fs::{create_dir_all, read, remove_dir, File};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Error, Pid};

#[derive(Clone, Debug)]
pub struct Cgroup {
    mount_path: PathBuf,
    path: PathBuf,
}

const PROC_CGROUP: &str = "/proc/self/cgroup";
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";
const CGROUP_PROCS: &str = "cgroup.procs";

impl Cgroup {
    pub fn new(mount_path: impl Into<PathBuf>, name: impl AsRef<Path>) -> Result<Self, Error> {
        let name = name.as_ref();
        if name.is_absolute() {
            Err("Cgroup name cannot be absolute")?
        }
        let mount_path = mount_path.into();
        if !mount_path.is_absolute() {
            Err("Cgroup mount path should be absolute")?
        }
        let path = mount_path.join(name);
        Ok(Self { mount_path, path })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &Path {
        self.path
            .strip_prefix(&self.mount_path)
            .expect("Cgroup path does not starts with mount path")
    }

    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }

    pub fn current() -> Result<Self, Error> {
        for line in String::from_utf8(read(PROC_CGROUP)?)?.split('\n') {
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
        if path.starts_with(&self.mount_path) {
            let mount_path = self.mount_path.clone();
            let path = path.to_owned();
            Some(Self { mount_path, path })
        } else {
            None
        }
    }

    pub fn child(&self, name: impl AsRef<Path>) -> Result<Self, Error> {
        let name = name.as_ref();
        if name.is_absolute() {
            Err("Child cgroup name cannot be absolute")?
        }
        let mount_path = self.mount_path.clone();
        let path = self.path.join(name);
        Ok(Self { mount_path, path })
    }

    pub fn create(&self) -> Result<(), Error> {
        Ok(create_dir_all(&self.path)?)
    }

    pub fn remove(&self) -> Result<(), Error> {
        Ok(remove_dir(&self.path)?)
    }

    pub fn add_process(&self, pid: Pid) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .truncate(false)
            .open(self.path.join(CGROUP_PROCS))?
            .write_all(pid.to_string().as_bytes())?;
        Ok(())
    }

    /// Reads current memory usage.
    pub fn memory_current(&self) -> Result<usize, Error> {
        let content = std::fs::read_to_string(self.path.join("memory.current"))?;
        Ok(content.trim_end().parse()?)
    }

    /// Reads peak memory usage.
    pub fn memory_peak(&self) -> Result<usize, Error> {
        let content = std::fs::read_to_string(self.path.join("memory.peak"))?;
        Ok(content.trim_end().parse()?)
    }

    pub fn memory_events(&self) -> Result<CgroupMemoryEvents, Error> {
        let content = std::fs::read(self.path.join("memory.events"))?;
        let mut events = CgroupMemoryEvents::default();
        for line in content.split(|c| *c == b'\n').filter(|v| !v.is_empty()) {
            let (key, value) = match std::str::from_utf8(line)?.split_once(' ') {
                Some(v) => v,
                None => continue,
            };
            match key {
                "low" => events.low = value.trim_end().parse()?,
                "high" => events.high = value.trim_end().parse()?,
                "max" => events.max = value.trim_end().parse()?,
                "oom" => events.oom = value.trim_end().parse()?,
                "oom_kill" => events.oom_kill = value.trim_end().parse()?,
                "oom_group_kill" => events.oom_group_kill = value.trim_end().parse()?,
                _ => continue,
            }
        }
        Ok(events)
    }

    pub fn set_memory_limit(&self, bytes: usize) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("memory.max"))?
            .write_all(format!("{}", bytes).as_bytes())?;
        Ok(())
    }

    pub fn set_memory_guarantee(&self, bytes: usize) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("memory.min"))?
            .write_all(format!("{}", bytes).as_bytes())?;
        Ok(())
    }

    pub fn set_swap_memory_limit(&self, limit: usize) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("memory.swap.max"))?
            .write_all(format!("{}", limit).as_bytes())?;
        Ok(())
    }

    pub fn cpu_usage(&self) -> Result<CgroupCpuUsage, Error> {
        let content = std::fs::read(self.path.join("cpu.stat"))?;
        let mut usage = CgroupCpuUsage::default();
        for line in content.split(|c| *c == b'\n').filter(|v| !v.is_empty()) {
            let (key, value) = match std::str::from_utf8(line)?.split_once(' ') {
                Some(v) => v,
                None => continue,
            };
            match key {
                "usage_usec" => usage.total = Duration::from_micros(value.trim_end().parse()?),
                "user_usec" => usage.user = Duration::from_micros(value.trim_end().parse()?),
                "system_usec" => usage.system = Duration::from_micros(value.trim_end().parse()?),
                _ => continue,
            }
        }
        Ok(usage)
    }

    pub fn set_cpu_limit(&self, limit: Duration, period: Duration) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("cpu.max"))?
            .write_all(format!("{} {}", limit.as_micros(), period.as_micros()).as_bytes())?;
        Ok(())
    }

    pub fn set_pids_limit(&self, limit: usize) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("pids.max"))?
            .write_all(format!("{}", limit).as_bytes())?;
        Ok(())
    }

    pub fn controllers(&self) -> Result<Vec<String>, Error> {
        let content = std::fs::read(self.path.join("cgroup.controllers"))?;
        let mut controllers = Vec::new();
        for line in content.split(|c| *c == b'\n').filter(|v| !v.is_empty()) {
            std::str::from_utf8(line)?
                .split(' ')
                .for_each(|v| controllers.push(v.to_owned()));
        }
        Ok(controllers)
    }

    pub fn subtree_controllers(&self) -> Result<Vec<String>, Error> {
        let content = std::fs::read(self.path.join("cgroup.subtree_control"))?;
        let mut controllers = Vec::new();
        for line in content.split(|c| *c == b'\n').filter(|v| !v.is_empty()) {
            std::str::from_utf8(line)?
                .split(' ')
                .for_each(|v| controllers.push(v.to_owned()));
        }
        Ok(controllers)
    }

    pub fn add_subtree_controllers(&self, controllers: Vec<String>) -> Result<(), Error> {
        File::options()
            .create(false)
            .write(true)
            .open(self.path.join("cgroup.subtree_control"))?
            .write_all(
                controllers
                    .into_iter()
                    .fold(String::new(), |acc, v| acc + " +" + &v)
                    .as_bytes(),
            )?;
        Ok(())
    }

    pub fn open(&self) -> Result<File, Error> {
        Ok(File::options()
            .read(true)
            .custom_flags(nix::libc::O_PATH | nix::libc::O_DIRECTORY)
            .open(&self.path)?)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CgroupMemoryEvents {
    pub low: usize,
    pub high: usize,
    pub max: usize,
    pub oom: usize,
    pub oom_kill: usize,
    pub oom_group_kill: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CgroupCpuUsage {
    pub total: Duration,
    pub user: Duration,
    pub system: Duration,
}
