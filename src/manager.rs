use std::fs::{create_dir, create_dir_all, remove_dir};
use std::io::ErrorKind;
use std::path::PathBuf;

use nix::unistd::{getgid, getuid};

use crate::{ignore_kind, Container, ContainerConfig, Gid, Uid};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct IdMap<T> {
    pub container_id: T,
    pub host_id: T,
    pub size: u32,
}

impl<T: From<nix::libc::uid_t>> IdMap<T> {
    pub(crate) fn new_container_root(host_id: T) -> Self {
        Self {
            host_id,
            container_id: 0.into(),
            size: 1,
        }
    }
}

pub struct Manager {
    state_path: PathBuf,
    cgroup_path: PathBuf,
    uid_map: Vec<IdMap<Uid>>,
    gid_map: Vec<IdMap<Gid>>,
}

impl Manager {
    pub fn new(
        state_path: impl Into<PathBuf>,
        cgroup_path: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let state_path = state_path.into();
        let cgroup_path = cgroup_path.into();
        assert!(cgroup_path.starts_with("/sys/fs/cgroup/"));
        ignore_kind(create_dir(&cgroup_path), ErrorKind::AlreadyExists)
            .map_err(|v| format!("Cannot create cgroup: {}", v))?;
        create_dir_all(&state_path).map_err(|v| format!("Cannot create state directory: {}", v))?;
        Ok(Self {
            state_path,
            cgroup_path,
            uid_map: vec![IdMap::new_container_root(getuid())],
            gid_map: vec![IdMap::new_container_root(getgid())],
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
        ignore_kind(remove_dir(&cgroup_path), ErrorKind::NotFound)?;
        create_dir(&cgroup_path).map_err(|v| format!("Cannot create cgroup: {}", v))?;
        if let Err(err) = create_dir(&state_path) {
            let _ = remove_dir(cgroup_path);
            return Err(format!("Cannot create state directory: {}", err).into());
        }
        ignore_kind(
            create_dir(state_path.join("rootfs")),
            ErrorKind::AlreadyExists,
        )
        .map_err(|v| format!("Cannot create rootfs: {}", v))?;
        ignore_kind(
            create_dir(state_path.join("diff")),
            ErrorKind::AlreadyExists,
        )
        .map_err(|v| format!("Cannot create overlay diff: {}", v))?;
        create_dir(state_path.join("work"))
            .map_err(|v| format!("Cannot create overlay work: {}", v))?;
        let container = Container {
            state_path,
            cgroup_path,
            uid_map: self.uid_map.clone(),
            gid_map: self.gid_map.clone(),
            config,
            pid: None,
        };
        Ok(container)
    }
}
