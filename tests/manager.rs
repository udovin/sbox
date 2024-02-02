use std::path::PathBuf;

use sbox::{ContainerConfig, Error, IdMap, Manager, ProcessConfig};

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
    let rootfs = get_rootfs().unwrap();
    let cgroup = get_cgroup().unwrap();
    let manager = Manager::new("/tmp/sbox", cgroup.clone()).unwrap();
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
    process.wait().unwrap();
    container.destroy().unwrap();
}
