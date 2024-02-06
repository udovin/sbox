use std::fs::{create_dir, create_dir_all, remove_dir, remove_dir_all};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::unistd::Uid;
use tar::Archive;

use crate::{ignore_kind, Container, ContainerConfig, Gid, RootFnTask, UserMapper};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Manager {
    state_path: PathBuf,
    cgroup_path: PathBuf,
    user_mapper: Arc<dyn UserMapper>,
}

impl Manager {
    pub fn new<SP, CP, UM>(state_path: SP, cgroup_path: CP, user_mapper: UM) -> Result<Self, Error>
    where
        SP: Into<PathBuf>,
        CP: Into<PathBuf>,
        UM: UserMapper + 'static,
    {
        let state_path = state_path.into();
        let cgroup_path = cgroup_path.into();
        assert!(cgroup_path.starts_with("/sys/fs/cgroup/"));
        ignore_kind(create_dir(&cgroup_path), ErrorKind::AlreadyExists)
            .map_err(|v| format!("Cannot create cgroup: {}", v))?;
        create_dir_all(&state_path).map_err(|v| format!("Cannot create state directory: {}", v))?;
        if user_mapper.uid_count() > 1 && !user_mapper.is_uid_mapped(Uid::from_raw(0)) {
            return Err("No mapping for root user".into());
        }
        if user_mapper.gid_count() > 1 && !user_mapper.is_gid_mapped(Gid::from_raw(0)) {
            return Err("No mapping for root group".into());
        }
        Ok(Self {
            state_path,
            cgroup_path,
            user_mapper: Arc::new(user_mapper),
        })
    }

    pub fn import_layer<R, P>(&self, mut archive: Archive<R>, path: P) -> Result<(), Error>
    where
        R: Read,
        P: AsRef<Path>,
    {
        RootFnTask::start(self.user_mapper.as_ref(), move || Ok(archive.unpack(path)?))
    }

    pub fn remove_layer<P>(&self, path: P) -> Result<(), Error>
    where
        P: AsRef<Path>,
    {
        RootFnTask::start(self.user_mapper.as_ref(), move || Ok(remove_dir_all(path)?))
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
            user_mapper: self.user_mapper.clone(),
            config,
            pid: None,
        };
        Ok(container)
    }
}
