use std::fs::{create_dir, remove_dir_all, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rand::distributions::{Alphanumeric, DistString};
use sbox::{
    run_as_root, BaseMounts, Cgroup, Container, Error, Gid, InitProcess, NewIdMap, OverlayMount,
    Process, Uid,
};
use tar::Archive;

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

    #[allow(unused)]
    pub fn release(self) -> Result<(), Error> {
        Ok(std::fs::remove_dir_all(&self.0)?)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

fn get_rootfs() -> Result<Archive<File>, Error> {
    let mut child = std::process::Command::new("/bin/sh")
        .arg("./get_rootfs.sh")
        .current_dir("./tests")
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
    let mut rootfs = Archive::new(File::open("./tests/rootfs.tar")?);
    rootfs.set_preserve_permissions(true);
    rootfs.set_preserve_ownerships(true);
    rootfs.set_unpack_xattrs(true);
    Ok(rootfs)
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
        return Ok(PathBuf::from("/sys/fs/cgroup")
            .join(
                parts
                    .get(2)
                    .ok_or("expected cgroup path")?
                    .trim_start_matches('/'),
            )
            .join("sbox-test"));
    }
    todo!()
}

#[test]
fn test_container() {
    let tmpdir = temp_dir().unwrap();
    let cgroup = get_cgroup().unwrap();
    let state_dir = tmpdir.join("state");
    let rootfs_dir = tmpdir.join("rootfs");
    let user_mapper = NewIdMap::new_root_subid(Uid::current(), Gid::current()).unwrap();
    {
        let rootfs_dir = rootfs_dir.clone();
        let mut rootfs = get_rootfs().unwrap();
        run_as_root(&user_mapper, move || Ok(rootfs.unpack(rootfs_dir)?)).unwrap();
    }
    println!("Rootfs path: {:?}", rootfs_dir);
    println!("Cgroup path: {:?}", cgroup);
    println!("State path: {:?}", state_dir);
    create_dir(&state_dir).unwrap();
    create_dir(state_dir.join("upper")).unwrap();
    create_dir(state_dir.join("work")).unwrap();
    let cgroup = Cgroup::new(
        "/sys/fs/cgroup",
        cgroup.strip_prefix("/sys/fs/cgroup").unwrap(),
    )
    .unwrap();
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
    cgroup.remove().unwrap();
    run_as_root(&user_mapper, move || Ok(remove_dir_all(tmpdir.as_path())?)).unwrap();
}
