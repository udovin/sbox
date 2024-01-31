use std::fs::{create_dir, create_dir_all, remove_dir};
use std::io::ErrorKind;
use std::path::PathBuf;

use super::{Container, ContainerConfig};

pub type Error = Box<dyn std::error::Error>;

pub struct Manager {
    state_path: PathBuf,
    cgroup_path: PathBuf,
}

impl Manager {
    pub fn new(
        state_path: impl Into<PathBuf>,
        cgroup_path: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let state_path = state_path.into();
        let cgroup_path = cgroup_path.into();
        assert!(cgroup_path.starts_with("/sys/fs/cgroup/"));
        create_dir_all(&state_path)?;
        if let Err(err) = create_dir(&cgroup_path) {
            if err.kind() != ErrorKind::AlreadyExists {
                return Err(format!("cannot create cgroup: {}", err).into());
            }
        }
        Ok(Self {
            state_path,
            cgroup_path,
        })
    }

    pub fn start_init_process() {
        todo!()
    }

    pub fn create_container(
        &self,
        id: String,
        config: ContainerConfig,
    ) -> Result<Container, Error> {
        let state_path = self.state_path.join(&id);
        let cgroup_path = self.cgroup_path.join(&id);
        create_dir(&cgroup_path)?;
        if let Err(err) = create_dir(&state_path) {
            let _ = remove_dir(cgroup_path);
            return Err(err.into());
        }
        let container = Container {
            state_path,
            cgroup_path,
            config,
        };
        Ok(container)
    }
}
