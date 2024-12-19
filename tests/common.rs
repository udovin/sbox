use std::{
    fs::File,
    io::ErrorKind,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Once,
};

use rand::distributions::{Alphanumeric, DistString as _};
use sbox::{Cgroup, Error};
use tar::Archive;

pub struct TempDir(PathBuf);

impl TempDir {
    #[allow(unused)]
    pub fn new() -> Result<Self, Error> {
        let tmpdir = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let path = loop {
            let path = tmpdir.join(format!("test-{}", rand_string(32)));
            match std::fs::metadata(&path) {
                Ok(_) => continue,
                Err(v) if v.kind() == ErrorKind::NotFound => break path,
                Err(v) => return Err(v.into()),
            }
        };
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    #[allow(unused)]
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.0.join(path)
    }

    #[allow(unused)]
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    #[allow(unused)]
    pub fn remove(self) -> Result<(), Error> {
        Ok(std::fs::remove_dir_all(&self.0)?)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[allow(unused)]
pub fn rand_string(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

#[allow(unused)]
pub fn get_rootfs() -> Result<Archive<File>, Error> {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        assert!(std::process::Command::new("curl")
            .arg("-fsSL")
            .arg("--retry")
            .arg("5")
            .arg("https://github.com/docker-library/busybox/raw/31d342ad033e27c18723a516a2274ab39547be27/stable/glibc/busybox.tar.xz")
            .arg("-o")
            .arg(format!("rootfs.tar.xz"))
            .current_dir("./tests")
            .spawn()
            .unwrap()
            .wait()
            .unwrap()
            .success());
        assert!(std::process::Command::new("xz")
            .arg("-df")
            .arg("rootfs.tar.xz")
            .current_dir("./tests")
            .spawn()
            .unwrap()
            .wait()
            .unwrap()
            .success());
    });
    let mut rootfs = Archive::new(File::open("./tests/rootfs.tar")?);
    rootfs.set_preserve_permissions(true);
    rootfs.set_preserve_ownerships(true);
    rootfs.set_unpack_xattrs(true);
    Ok(rootfs)
}

#[allow(unused)]
pub fn get_cgroup() -> Result<Cgroup, Error> {
    if let Ok(v) = std::env::var("TEST_CGROUP_PATH") {
        let path = PathBuf::from(v);
        let root_path = "/sys/fs/cgroup";
        return Cgroup::new(root_path, path.strip_prefix(root_path).unwrap());
    }
    Ok(Cgroup::current()?
        .parent()
        .ok_or("Current process cannot be in root cgroup")?)
}

pub struct TempCgroup(Cgroup);

impl TempCgroup {
    #[allow(unused)]
    pub fn new() -> Result<Self, Error> {
        let cgroup = get_cgroup()?.child(format!("test-{}", rand_string(32)))?;
        cgroup.create()?;
        Ok(Self(cgroup))
    }
}

impl Deref for TempCgroup {
    type Target = Cgroup;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TempCgroup {
    fn drop(&mut self) {
        let _ = self.0.remove();
    }
}
