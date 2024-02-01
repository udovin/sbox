use std::fs::{remove_dir, remove_dir_all, File};
use std::io::Write;
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
    pub fn kill(&self) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .open(self.cgroup_path.join("cgroup.kill"))?;
        file.write("1".as_bytes())?;
        drop(file);
        Ok(())
    }

    /// Releases all associated resources with container.
    pub fn destroy(self) -> Result<(), Error> {
        let kill_err = self.kill();
        let state_err = self.remove_state();
        let cgroup_err = remove_dir(&self.cgroup_path);
        kill_err?;
        state_err?;
        Ok(cgroup_err?)
    }

    fn remove_state(&self) -> Result<(), Error> {
        remove_dir(&self.state_path.join("work/work"))?;
        remove_dir(&self.state_path.join("work"))?;
        Ok(remove_dir_all(&self.state_path)?)
    }
}
