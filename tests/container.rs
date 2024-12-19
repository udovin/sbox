use std::fs::{create_dir, remove_dir_all, File};
use std::io::Read;

use common::{get_rootfs, TempCgroup, TempDir};
use sbox::{
    run_as_root, BaseMounts, BinNewIdMapper, Container, Gid, InitProcess, OverlayMount, Process,
    Slirp4NetnsManager, Uid,
};

mod common;

#[test]
fn test_container() {
    let tmpdir = TempDir::new().unwrap();
    let cgroup = TempCgroup::new().unwrap();
    let state_dir = tmpdir.join("state");
    let rootfs_dir = tmpdir.join("rootfs");
    let user_mapper = BinNewIdMapper::new_root_subid(Uid::current(), Gid::current()).unwrap();
    {
        let rootfs_dir = rootfs_dir.clone();
        let mut rootfs = get_rootfs().unwrap();
        run_as_root(&user_mapper, move || Ok(rootfs.unpack(rootfs_dir)?)).unwrap();
    }
    println!("Rootfs path: {:?}", rootfs_dir);
    println!("Cgroup path: {:?}", cgroup.as_path());
    println!("State path: {:?}", state_dir);
    create_dir(&state_dir).unwrap();
    create_dir(state_dir.join("upper")).unwrap();
    create_dir(state_dir.join("work")).unwrap();
    let container = Container::options()
        .cgroup(cgroup.clone())
        .add_mount(OverlayMount::new(
            vec![rootfs_dir.clone()],
            state_dir.join("upper"),
            state_dir.join("work"),
        ))
        .add_mount(BaseMounts::new())
        .rootfs(state_dir.join("rootfs"))
        .user_mapper(user_mapper.clone())
        .network_manager(Slirp4NetnsManager::new())
        .create()
        .unwrap();
    let mut init_process = InitProcess::options()
        .command(vec![
            "/bin/sh".into(),
            "-c".into(),
            "id && cat /proc/self/cgroup && ls -al /proc/self/ns && sleep 2 && echo 'Init exited'"
                .into(),
        ])
        .cgroup("init")
        .start(&container)
        .unwrap();
    Process::options()
        .command(vec![
            "/bin/sh".into(),
            "-c".into(),
            "id && cat /proc/self/cgroup && ls -al /proc/self/ns && adduser -D -u1000 user && echo 'System exited'".into(),
        ])
        .cgroup("system")
        .start(&container, &init_process)
        .unwrap()
        .wait()
        .unwrap();
    Process::options()
        .command(vec![
            "/bin/sh".into(),
            "-c".into(),
            "id && cat /proc/self/cgroup && ls -al /proc/self/ns && echo 'User exited'".into(),
        ])
        .cgroup("user")
        .user(1000, 1000)
        .start(&container, &init_process)
        .unwrap()
        .wait()
        .unwrap();
    init_process.wait().unwrap();
    cgroup.child("init").unwrap().remove().unwrap();
    cgroup.child("system").unwrap().remove().unwrap();
    cgroup.child("user").unwrap().remove().unwrap();
    run_as_root(&user_mapper, move || Ok(remove_dir_all(tmpdir.as_path())?)).unwrap();
}

#[test]
fn test_container_stdio() {
    let tmpdir = TempDir::new().unwrap();
    let cgroup = TempCgroup::new().unwrap();
    let state_dir = tmpdir.join("state");
    let rootfs_dir = tmpdir.join("rootfs");
    let user_mapper = BinNewIdMapper::new_root_subid(Uid::current(), Gid::current()).unwrap();
    {
        let rootfs_dir = rootfs_dir.clone();
        let mut rootfs = get_rootfs().unwrap();
        run_as_root(&user_mapper, move || Ok(rootfs.unpack(rootfs_dir)?)).unwrap();
    }
    println!("Rootfs path: {:?}", rootfs_dir);
    println!("Cgroup path: {:?}", cgroup.as_path());
    println!("State path: {:?}", state_dir);
    create_dir(&state_dir).unwrap();
    create_dir(state_dir.join("upper")).unwrap();
    create_dir(state_dir.join("work")).unwrap();
    let container = Container::options()
        .cgroup(cgroup.clone())
        .add_mount(OverlayMount::new(
            vec![rootfs_dir.clone()],
            state_dir.join("upper"),
            state_dir.join("work"),
        ))
        .add_mount(BaseMounts::new())
        .rootfs(state_dir.join("rootfs"))
        .user_mapper(user_mapper.clone())
        .create()
        .unwrap();
    let (rx, tx) = nix::unistd::pipe().unwrap();
    let (erx, etx) = nix::unistd::pipe().unwrap();
    let mut init_process = InitProcess::options()
        .command(vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo 'example stdout' && echo 'example stderr' >&2".into(),
        ])
        .stdout(tx)
        .stderr(etx)
        .start(&container)
        .unwrap();
    let mut stdout = String::new();
    File::from(rx).read_to_string(&mut stdout).unwrap();
    let mut stderr = String::new();
    File::from(erx).read_to_string(&mut stderr).unwrap();
    init_process.wait().unwrap();
    assert_eq!(stdout, "example stdout\n");
    assert_eq!(stderr, "example stderr\n");
}

#[test]
fn test_cgroup() {
    let cgroup = TempCgroup::new().unwrap();
    {
        let controllers = cgroup.subtree_controllers().unwrap();
        assert!(controllers.is_empty(), "{controllers:#?}");
    }
    cgroup
        .add_subtree_controllers(vec!["cpu".into(), "memory".into(), "pids".into()])
        .unwrap();
    {
        let mut controllers = cgroup.subtree_controllers().unwrap();
        controllers.sort();
        assert_eq!(controllers, ["cpu", "memory", "pids"]);
    }
}
