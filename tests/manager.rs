use sbox::{ContainerConfig, IdMap, Manager, ProcessConfig};

#[test]
fn test_manager() {
    let manager = Manager::new(
        "/tmp/sbox",
        "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/user.slice/test-sbox",
    )
    .unwrap();
    let container_config = ContainerConfig {
        layers: vec!["/tmp/sbox-rootfs".into()],
        uid_map: vec![
            IdMap {
                container_id: 0,
                host_id: nix::unistd::getuid().as_raw(),
                size: 1,
            },
            IdMap {
                container_id: 1,
                host_id: 100000,
                size: 65536,
            },
        ],
        gid_map: vec![
            IdMap {
                container_id: 0,
                host_id: nix::unistd::getuid().as_raw(),
                size: 1,
            },
            IdMap {
                container_id: 1,
                host_id: 100000,
                size: 65536,
            },
        ],
        ..Default::default()
    };
    let container = manager
        .create_container("test1".into(), container_config)
        .unwrap();
    let process_config = ProcessConfig {
        command: vec!["/bin/bash2".into()],
        ..Default::default()
    };
    let process = container.start(process_config).unwrap();
    process.wait().unwrap();
}
