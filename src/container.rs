use std::fs::create_dir_all;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{Cgroup, Mount, NetworkManager, UserMapper};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct ContainerOptions {
    rootfs: Option<PathBuf>,
    cgroup: Option<Cgroup>,
    user_mapper: Option<Arc<dyn UserMapper>>,
    network_manager: Option<Arc<dyn NetworkManager>>,
    mounts: Vec<Arc<dyn Mount>>,
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

    pub fn user_mapper<T: UserMapper + 'static>(mut self, user_mapper: T) -> Self {
        self.user_mapper = Some(Arc::new(user_mapper));
        self
    }

    pub fn network_manager<T: NetworkManager + 'static>(mut self, network_manager: T) -> Self {
        self.network_manager = Some(Arc::new(network_manager));
        self
    }

    pub fn add_mount<T: Mount + 'static>(mut self, mount: T) -> Self {
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
        let network_manager = self.network_manager;
        let mounts = self.mounts;
        let hostname = self.hostname;
        create_dir_all(&rootfs)?;
        cgroup.create()?;
        Ok(Container {
            rootfs,
            cgroup,
            user_mapper,
            network_manager,
            mounts,
            hostname,
        })
    }
}

pub struct Container {
    pub(super) rootfs: PathBuf,
    pub(super) cgroup: Cgroup,
    pub(super) user_mapper: Arc<dyn UserMapper>,
    pub(super) network_manager: Option<Arc<dyn NetworkManager>>,
    pub(super) mounts: Vec<Arc<dyn Mount>>,
    pub(super) hostname: String,
}

impl Container {
    pub fn options() -> ContainerOptions {
        ContainerOptions::new()
    }
}
