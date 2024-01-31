use std::fs::{remove_dir, remove_dir_all};
use std::path::PathBuf;

use super::{Error, Process, ProcessConfig};

#[derive(Debug)]
pub struct IdMap {
    pub container_id: u32,
    pub host_id: u32,
    pub size: u32,
}

#[derive(Debug, Default)]
pub struct ContainerConfig {
    pub layers: Vec<PathBuf>,
    pub uid_map: Vec<IdMap>,
    pub gid_map: Vec<IdMap>,
}

pub struct Container {
    pub(super) state_path: PathBuf,
    pub(super) cgroup_path: PathBuf,
    pub(super) config: ContainerConfig,
}

impl Container {
    /// Starts container with initial process.
    pub fn start(&self, config: ProcessConfig) -> Result<Process, Error> {
        let mut process = Process { pid: None, config };
        process.init_container(&self)?;
        Ok(process)
    }

    /// Executes process inside container.
    pub fn execute(&self, config: ProcessConfig) -> Result<Process, Error> {
        let mut process = Process { pid: None, config };
        process.execute(&self)?;
        Ok(process)
    }

    /// Kills all processes inside container.
    pub fn kill_all(&self) -> Result<(), Error> {
        todo!()
    }

    /// Releases all associated resources with container.
    pub fn destroy(self) -> Result<(), Error> {
        let _ = self.kill_all();
        let state_err = remove_dir_all(&self.state_path);
        let cgroup_err = remove_dir(&self.cgroup_path);
        state_err?;
        Ok(cgroup_err?)
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.state_path);
        let _ = remove_dir(&self.cgroup_path);
    }
}
