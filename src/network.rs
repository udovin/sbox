use std::fmt::Debug;
use std::fs::File;
use std::io::Write as _;
use std::panic::RefUnwindSafe;
use std::path::PathBuf;

use crate::{Error, Pid};

pub trait NetworkHandle: Send + Sync + Debug + RefUnwindSafe {}

pub trait NetworkManager: Send + Sync + Debug + RefUnwindSafe {
    fn run_network(&self, pid: Pid) -> Result<Option<Box<dyn NetworkHandle>>, Error>;

    fn set_network(&self) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct Slirp4NetnsManager {
    pub binary: PathBuf,
}

impl Slirp4NetnsManager {
    pub fn new() -> Self {
        Self {
            binary: "/bin/slirp4netns".into(),
        }
    }
}

impl Default for Slirp4NetnsManager {
    fn default() -> Self {
        Slirp4NetnsManager::new()
    }
}

impl NetworkManager for Slirp4NetnsManager {
    fn run_network(&self, pid: Pid) -> Result<Option<Box<dyn NetworkHandle>>, Error> {
        let handle = std::process::Command::new(&self.binary)
            .arg("--configure")
            .arg("--mtu=65520")
            .arg("--disable-host-loopback")
            .arg(pid.to_string())
            .arg("tap0")
            .spawn()?;
        Ok(Some(Box::new(Slirp4NetnsHandle { handle })))
    }

    fn set_network(&self) -> Result<(), Error> {
        Ok(File::create("/etc/resolv.conf")?.write_all("nameserver 10.0.2.3".as_bytes())?)
    }
}

#[derive(Debug)]
pub struct Slirp4NetnsHandle {
    handle: std::process::Child,
}

impl NetworkHandle for Slirp4NetnsHandle {}

impl Drop for Slirp4NetnsHandle {
    fn drop(&mut self) {
        let _ = self.handle.kill();
        let _ = self.handle.wait();
    }
}
