# sbox

[![crates.io](https://img.shields.io/crates/v/sbox.svg)](https://crates.io/crates/sbox)
[![codecov](https://codecov.io/gh/udovin/sbox/graph/badge.svg?token=rSCoZyJyKV)](https://codecov.io/gh/udovin/sbox)

Tiny Linux containers implementation.

## Usage

```rust
use std::fs::create_dir_all;
use std::path::PathBuf;

use nix::unistd::{getgid, getuid};
use sbox::{BaseMounts, Cgroup, Container, InitProcess, NewIdMap, OverlayMount};

fn main() {
    // Create user namespace mapper for current user with subuids and subgids.
    let user_mapper = NewIdMap::new_root_subid(getuid(), getgid()).unwrap();
    // Create cgroup for container.
    let cgroup = Cgroup::new("/sys/fs/cgroup", "sbox").unwrap();
    // Path to rootfs for container image.
    let image_dir = PathBuf::from("/tmp/sbox-image");
    // Path to container state dir.
    let state_dir = PathBuf::from("/tmp/sbox-state");
    create_dir_all(state_dir.join("upper")).unwrap();
    create_dir_all(state_dir.join("work")).unwrap();
    // Create container.
    let container = Container::options()
        .cgroup(cgroup)
        .add_mount(OverlayMount::new(
            vec![image_dir],
            state_dir.join("upper"),
            state_dir.join("work"),
        ))
        .add_mount(BaseMounts::new())
        .rootfs(state_dir.join("rootfs"))
        .user_mapper(user_mapper.clone())
        .create()
        .unwrap();
    // Start container.
    InitProcess::options()
        .command(vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo 'Hello, World' && id && cat /proc/self/cgroup".into(),
        ])
        .start(&container)
        .unwrap()
        .wait()
        .unwrap();
}
```

## License

sbox is distributed under the terms of both the MIT license and the Apache 2.0 License.
