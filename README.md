# sbox

[![crates.io](https://img.shields.io/crates/v/sbox.svg)](https://crates.io/crates/sbox)
[![codecov](https://codecov.io/gh/udovin/sbox-rs/graph/badge.svg?token=1e6dCU26Nq)](https://codecov.io/gh/udovin/sbox-rs)

Tiny Linux containers implementation.

## Usage

```rust
fn main() {
    // Create container manager.
    let manager = Manager::new("/tmp/sbox", "/sys/fs/cgroup/sbox").unwrap();
    // Create container.
    let mut container = manager
        .create_container(
            "example".into(),
            ContainerConfig {
                layers: vec!["/tmp/sbox-rootfs".into()],
                ..Default::default()
            },
        )
        .unwrap();
    // Start container.
    let process = container
        .start(ProcessConfig {
            command: vec!["/bin/sh".into(), "-c".into(), "echo 'Hello, World!'".into()],
            ..Default::default()
        })
        .unwrap();
    // Wait for init process exit.
    process.wait(None).unwrap();
    // Remove all container resources.
    container.destroy().unwrap();
}
```

## License

sbox is distributed under the terms of both the MIT license and the Apache 2.0 License.
