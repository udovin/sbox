use std::fs::create_dir_all;
use std::panic::RefUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{Cgroup, Mount, UserMapper};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct ContainerOptions {
    rootfs: Option<PathBuf>,
    cgroup: Option<Cgroup>,
    user_mapper: Option<Arc<dyn UserMapper + RefUnwindSafe>>,
    mounts: Vec<Arc<dyn Mount + RefUnwindSafe>>,
    hostname: String,
}

impl ContainerOptions {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn rootfs(mut self, rootfs: PathBuf) -> Self {
        self.rootfs = Some(rootfs);
        self
    }

    pub fn cgroup(mut self, cgroup: Cgroup) -> Self {
        self.cgroup = Some(cgroup);
        self
    }

    pub fn user_mapper<T: UserMapper + RefUnwindSafe + 'static>(mut self, user_mapper: T) -> Self {
        self.user_mapper = Some(Arc::new(user_mapper));
        self
    }

    pub fn add_mount<T: Mount + RefUnwindSafe + 'static>(mut self, mount: T) -> Self {
        self.mounts.push(Arc::new(mount));
        self
    }

    pub fn hostname<T: ToString>(mut self, hostname: T) -> Self {
        self.hostname = hostname.to_string();
        self
    }

    pub fn create(self) -> Result<Container, Error> {
        let rootfs = self.rootfs.ok_or("Container rootfs should specified")?;
        let cgroup = self.cgroup.ok_or("Container cgroup should specified")?;
        let user_mapper = self
            .user_mapper
            .ok_or("Container user mapper should specified")?;
        let mounts = self.mounts;
        let hostname = self.hostname;
        create_dir_all(&rootfs)?;
        cgroup.create()?;
        Ok(Container {
            rootfs,
            cgroup,
            user_mapper,
            mounts,
            hostname,
        })
    }
}

pub struct Container {
    pub(super) rootfs: PathBuf,
    pub(super) cgroup: Cgroup,
    pub(super) user_mapper: Arc<dyn UserMapper + RefUnwindSafe>,
    pub(super) mounts: Vec<Arc<dyn Mount + RefUnwindSafe>>,
    pub(super) hostname: String,
}

impl Container {
    pub fn options() -> ContainerOptions {
        ContainerOptions::new()
    }
}
