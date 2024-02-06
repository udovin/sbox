use std::fs::File;
use std::io::{BufRead, BufReader};
use std::process::Command;
use std::str::FromStr;

use nix::libc::uid_t;
use nix::unistd::{getgid, getuid, User};

use crate::{Error, Pid};

pub type Uid = nix::unistd::Uid;
pub type Gid = nix::unistd::Gid;

#[derive(Clone, Debug)]
pub struct IdMap<T> {
    pub container_id: T,
    pub host_id: T,
    pub size: u32,
}

impl<T: From<uid_t>> IdMap<T> {
    pub fn new_root(host_id: T) -> Self {
        Self {
            host_id,
            container_id: 0.into(),
            size: 1,
        }
    }
}

pub trait UserMapper {
    fn run_map_user(&self, pid: Pid) -> Result<(), Error>;

    fn is_uid_mapped(&self, id: Uid) -> bool;

    fn is_gid_mapped(&self, id: Gid) -> bool;

    fn uid_count(&self) -> u32;

    fn gid_count(&self) -> u32;
}

#[derive(Clone, Debug)]
pub struct NewIdMap {
    pub uid_map: Vec<IdMap<Uid>>,
    pub gid_map: Vec<IdMap<Gid>>,
    pub uid_binary: String,
    pub gid_binary: String,
}

impl NewIdMap {
    // Maps uid and gid as container root.
    pub fn new_root(uid: Uid, gid: Gid) -> Self {
        Self {
            uid_map: vec![IdMap::new_root(uid)],
            gid_map: vec![IdMap::new_root(gid)],
            uid_binary: "/bin/newuidmap".to_owned(),
            gid_binary: "/bin/newgidmap".to_owned(),
        }
    }

    // Maps uid and gid as container root, subuid and subgid as other users.
    pub fn new_root_subid(uid: Uid, gid: Gid) -> Result<Self, Error> {
        let user = match User::from_uid(uid)? {
            Some(v) => v,
            None => return Err(format!("Unknown user: {uid}").into()),
        };
        Ok(Self {
            uid_map: Self::get_id_subid_map("/etc/subuid", uid, &user)?,
            gid_map: Self::get_id_subid_map("/etc/subgid", gid, &user)?,
            uid_binary: "/bin/newuidmap".to_owned(),
            gid_binary: "/bin/newgidmap".to_owned(),
        })
    }

    fn get_id_subid_map<T>(path: &str, id: T, user: &User) -> Result<Vec<IdMap<T>>, Error>
    where
        T: Copy + From<uid_t> + Into<uid_t>,
    {
        Ok(match Self::find_subid(path, user)? {
            Some(v) => vec![
                IdMap::new_root(id),
                IdMap {
                    container_id: 1.into(),
                    host_id: v.0,
                    size: v.1,
                },
            ],
            None => vec![IdMap::new_root(id)],
        })
    }

    fn find_subid<T>(path: &str, user: &User) -> Result<Option<(T, u32)>, Error>
    where
        T: From<uid_t>,
    {
        let file = BufReader::new(File::open(path)?);
        for line in file.lines() {
            let line = line?;
            let parts: Vec<_> = line.split(':').collect();
            if parts.len() >= 3 && parts[0] == user.name {
                let start = uid_t::from_str(parts[1])?;
                let size = u32::from_str(parts[2])?;
                return Ok(Some((start.into(), size)));
            }
        }
        Ok(None)
    }

    fn run_id_map<T>(id_map: &[IdMap<T>], binary: &str, pid: Pid) -> Result<(), Error>
    where
        T: Copy + Into<uid_t>,
    {
        let mut cmd = Command::new(binary);
        cmd.arg(pid.as_raw().to_string());
        for v in id_map {
            cmd.arg(v.container_id.into().to_string())
                .arg(v.host_id.into().to_string())
                .arg(v.size.to_string());
        }
        let mut child = cmd.spawn()?;
        let status = child.wait()?;
        if !status.success() {
            let code = status.code().unwrap_or(0);
            return Err(format!("{binary} exited with code {code}").into());
        }
        Ok(())
    }

    fn is_mapped<T>(id_map: &[IdMap<T>], id: T) -> bool
    where
        T: Copy + Into<uid_t>,
    {
        for v in id_map {
            if v.container_id.into() + v.size <= id.into() {
                continue;
            }
            if v.container_id.into() <= id.into() {
                return true;
            }
        }
        false
    }
}

impl Default for NewIdMap {
    fn default() -> Self {
        Self::new_root(getuid(), getgid())
    }
}

impl UserMapper for NewIdMap {
    fn run_map_user(&self, pid: Pid) -> Result<(), Error> {
        Self::run_id_map(&self.uid_map, &self.uid_binary, pid)?;
        Self::run_id_map(&self.gid_map, &self.gid_binary, pid)?;
        Ok(())
    }

    fn is_uid_mapped(&self, uid: Uid) -> bool {
        Self::is_mapped(&self.uid_map, uid)
    }

    fn is_gid_mapped(&self, gid: Gid) -> bool {
        Self::is_mapped(&self.gid_map, gid)
    }

    fn uid_count(&self) -> u32 {
        self.uid_map.iter().fold(0, |acc, x| acc + x.size)
    }

    fn gid_count(&self) -> u32 {
        self.gid_map.iter().fold(0, |acc, x| acc + x.size)
    }
}
