use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rand::distributions::{Alphanumeric, DistString};
use sbox::{ContainerConfig, Error, Manager, ProcessConfig};

pub struct TempDir(PathBuf);

impl TempDir {
    #[allow(unused)]
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.0.join(path)
    }

    #[allow(unused)]
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[allow(unused)]
pub fn temp_dir() -> Result<TempDir, Error> {
    let tmpdir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = loop {
        let path = tmpdir.join(Alphanumeric.sample_string(&mut rand::thread_rng(), 32));
        match std::fs::metadata(&path) {
            Ok(_) => continue,
            Err(v) if v.kind() == ErrorKind::NotFound => break path,
            Err(v) => return Err(v.into()),
        }
    };
    std::fs::create_dir_all(&path)?;
    Ok(TempDir(path))
}

fn get_rootfs() -> Result<PathBuf, Error> {
    let mut child = std::process::Command::new("/bin/sh")
        .arg("./get_rootfs.sh")
        .current_dir("./tests")
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
    Ok(PathBuf::from("./tests/rootfs").canonicalize()?)
}

fn get_cgroup() -> Result<PathBuf, Error> {
    if let Ok(v) = std::env::var("TEST_CGROUP_PATH") {
        return Ok(v.into());
    }
    for line in String::from_utf8(std::fs::read("/proc/self/cgroup")?)?.split('\n') {
        let parts: Vec<_> = line.split(':').collect();
        if let Some(v) = parts.get(1) {
            if !v.is_empty() {
                continue;
            }
        }
        return Ok(PathBuf::from("/sys/fs/cgroup").join(
            parts
                .get(2)
                .ok_or("expected cgroup path")?
                .trim_start_matches('/'),
        ));
    }
    todo!()
}

#[test]
fn test_manager() {
    let tmpdir = temp_dir().unwrap();
    let rootfs = get_rootfs().unwrap();
    let cgroup = get_cgroup().unwrap();
    let state_dir = tmpdir.join("state");
    println!("Rootfs path: {:?}", rootfs);
    println!("Cgroup path: {:?}", cgroup);
    println!("State path: {:?}", state_dir);
    let manager = Manager::new(state_dir, cgroup.clone()).unwrap();
    let container_config = ContainerConfig {
        layers: vec![rootfs],
        ..Default::default()
    };
    let mut container = manager
        .create_container("test1".into(), container_config)
        .unwrap();
    let process_config = ProcessConfig {
        command: vec!["/bin/sh".into(), "-c".into(), "echo 'Hello, World!'".into()],
        ..Default::default()
    };
    let process = container.start(process_config).unwrap();
    process.wait(None).unwrap();
    container.destroy().unwrap();
}
