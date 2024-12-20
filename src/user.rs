use std::ffi::CString;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::panic::{catch_unwind, RefUnwindSafe, UnwindSafe};
use std::process::Command;
use std::str::FromStr;

use nix::libc::uid_t;
use nix::unistd::{getgid, getgrouplist, getuid, setgid, setgroups, setuid, User};

use crate::{
    clone3, exit_child, new_pipe, read_ok, read_result, write_ok, write_result, CloneArgs,
    CloneResult, Error, OwnedPid, Pid,
};

pub type Uid = nix::unistd::Uid;
pub type Gid = nix::unistd::Gid;

/// Represents mapping for IDs from host namespace to container namespace.
#[derive(Clone, Debug)]
pub struct IdMap<T> {
    /// First ID in container namespace.
    pub container_id: T,
    /// First ID in host namespace.
    pub host_id: T,
    /// Amount of mapped IDs.
    pub size: u32,
}

impl<T: From<uid_t>> IdMap<T> {
    /// Maps specified host ID as root (ID = 0) in container namespace.
    pub fn new_root(host_id: T) -> Self {
        Self {
            host_id,
            container_id: 0.into(),
            size: 1,
        }
    }
}

/// Represents mapper for user IDs and group IDs in container namespace.
pub trait UserMapper: Send + Sync + Debug + RefUnwindSafe {
    /// Runs mapping for new user namespace initialized by specified process.
    fn run_map_user(&self, pid: Pid) -> Result<(), Error>;

    /// Sets user ID and group ID for current process in user namespace.
    fn set_user(&self, uid: Uid, gid: Gid) -> Result<(), Error>;

    /// Verifies that specified user ID is represented in container.
    fn is_uid_mapped(&self, id: Uid) -> bool;

    /// Verifies that specified group ID is represented in container.
    fn is_gid_mapped(&self, id: Gid) -> bool;

    /// Calculates amount of mapped user IDs.
    fn uid_count(&self) -> u32;

    /// Calculates amount of mapped group IDs.
    fn gid_count(&self) -> u32;
}

#[derive(Clone, Debug)]
pub struct ProcUserMapper {
    pub uid_map: Vec<IdMap<Uid>>,
    pub gid_map: Vec<IdMap<Gid>>,
    pub set_groups: bool,
}

impl ProcUserMapper {
    /// Maps uid and gid as container root.
    pub fn new_root(uid: Uid, gid: Gid) -> Self {
        Self {
            uid_map: vec![IdMap::new_root(uid)],
            gid_map: vec![IdMap::new_root(gid)],
            set_groups: false,
        }
    }
}

/// Creates user mapper for current process uid and gid.
impl Default for ProcUserMapper {
    fn default() -> Self {
        Self::new_root(getuid(), getgid())
    }
}

impl UserMapper for ProcUserMapper {
    /// Runs mapping for new user namespace initialized by specified process.
    fn run_map_user(&self, _pid: Pid) -> Result<(), Error> {
        todo!()
    }

    /// Sets user ID and group ID for current process in user namespace.
    fn set_user(&self, uid: Uid, gid: Gid) -> Result<(), Error> {
        if self.set_groups {
            let groups = match User::from_uid(uid)? {
                Some(user) => getgrouplist(&CString::new(user.name.as_bytes())?, gid)?,
                None => vec![gid],
            };
            setgroups(&groups)?;
        }
        setgid(gid)?;
        Ok(setuid(uid)?)
    }

    /// Verifies that specified user ID is represented in container.
    fn is_uid_mapped(&self, uid: Uid) -> bool {
        is_id_mapped(&self.uid_map, uid)
    }

    /// Verifies that specified group ID is represented in container.
    fn is_gid_mapped(&self, gid: Gid) -> bool {
        is_id_mapped(&self.gid_map, gid)
    }

    /// Calculates amount of mapped user IDs.
    fn uid_count(&self) -> u32 {
        self.uid_map.iter().fold(0, |acc, x| acc + x.size)
    }

    /// Calculates amount of mapped group IDs.
    fn gid_count(&self) -> u32 {
        self.gid_map.iter().fold(0, |acc, x| acc + x.size)
    }
}

/// Represents user mapper implemented using new{u,g}idmap.
///
/// Uses new{u,g}idmap binaries from following paths:
///   * `/bin/newuidmap`
///   * `/bin/newgidmap`
#[derive(Clone, Debug)]
pub struct BinNewIdMapper {
    pub uid_map: Vec<IdMap<Uid>>,
    pub gid_map: Vec<IdMap<Gid>>,
    pub uid_binary: String,
    pub gid_binary: String,
}

impl BinNewIdMapper {
    /// Maps uid and gid as container root.
    ///
    /// Uses new{u,g}idmap binaries from following paths:
    ///   * `/bin/newuidmap`
    ///   * `/bin/newgidmap`
    pub fn new_root(uid: Uid, gid: Gid) -> Self {
        Self {
            uid_map: vec![IdMap::new_root(uid)],
            gid_map: vec![IdMap::new_root(gid)],
            uid_binary: "/bin/newuidmap".to_owned(),
            gid_binary: "/bin/newgidmap".to_owned(),
        }
    }

    /// Maps uid and gid as container root, subuid and subgid as other users.
    ///
    /// Uses new{u,g}idmap binaries from following paths:
    ///   * `/bin/newuidmap`
    ///   * `/bin/newgidmap`
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
}

/// Creates user mapper for current process uid and gid.
impl Default for BinNewIdMapper {
    fn default() -> Self {
        Self::new_root(getuid(), getgid())
    }
}

impl UserMapper for BinNewIdMapper {
    /// Runs mapping for new user namespace initialized by specified process.
    fn run_map_user(&self, pid: Pid) -> Result<(), Error> {
        Self::run_id_map(&self.uid_map, &self.uid_binary, pid)
            .map_err(|v| format!("Cannot map users: {v}"))?;
        Self::run_id_map(&self.gid_map, &self.gid_binary, pid)
            .map_err(|v| format!("Cannot map groups: {v}"))?;
        Ok(())
    }

    /// Sets user ID and group ID for current process in user namespace.
    fn set_user(&self, uid: Uid, gid: Gid) -> Result<(), Error> {
        let groups = match User::from_uid(uid)? {
            Some(user) => getgrouplist(&CString::new(user.name.as_bytes())?, gid)?,
            None => vec![gid],
        };
        setgroups(&groups).map_err(|v| format!("Cannot set groups: {v}"))?;
        setgid(gid).map_err(|v| format!("Cannot set group: {v}"))?;
        Ok(setuid(uid).map_err(|v| format!("Cannot set user: {v}"))?)
    }

    /// Verifies that specified user ID is represented in container.
    fn is_uid_mapped(&self, uid: Uid) -> bool {
        is_id_mapped(&self.uid_map, uid)
    }

    /// Verifies that specified group ID is represented in container.
    fn is_gid_mapped(&self, gid: Gid) -> bool {
        is_id_mapped(&self.gid_map, gid)
    }

    /// Calculates amount of mapped user IDs.
    fn uid_count(&self) -> u32 {
        self.uid_map.iter().fold(0, |acc, x| acc + x.size)
    }

    /// Calculates amount of mapped group IDs.
    fn gid_count(&self) -> u32 {
        self.gid_map.iter().fold(0, |acc, x| acc + x.size)
    }
}

pub fn run_as_user<
    T: UserMapper + RefUnwindSafe + ?Sized,
    Fn: FnOnce() -> Result<(), Error> + UnwindSafe,
>(
    user_mapper: &T,
    uid: impl Into<Uid> + UnwindSafe,
    gid: impl Into<Gid> + UnwindSafe,
    func: Fn,
) -> Result<(), Error> {
    let pipe = new_pipe()?;
    let child_pipe = new_pipe()?;
    let mut clone_args = CloneArgs::default();
    clone_args.flag_newuser();
    match unsafe { clone3(&clone_args) }? {
        CloneResult::Child => {
            let _ = catch_unwind(move || {
                let rx = pipe.rx();
                let tx = child_pipe.tx();
                exit_child(move || -> Result<(), Error> {
                    read_ok(rx)?;
                    write_result(
                        tx,
                        user_mapper
                            .set_user(uid.into(), gid.into())
                            .and_then(|_| func()),
                    )?
                }())
            });
            unsafe { nix::libc::_exit(2) }
        }
        CloneResult::Parent { child } => {
            let child = unsafe { OwnedPid::from_raw(child) };
            let rx = child_pipe.rx();
            let tx = pipe.tx();
            user_mapper.run_map_user(child.as_raw())?;
            // Unlock child process.
            write_ok(tx)?;
            // Await child process result.
            read_result(rx)??;
            child.wait_success()
        }
    }
}

pub fn run_as_root<
    T: UserMapper + RefUnwindSafe + ?Sized,
    Fn: FnOnce() -> Result<(), Error> + UnwindSafe,
>(
    user_mapper: &T,
    func: Fn,
) -> Result<(), Error> {
    run_as_user(user_mapper, 0, 0, func)
}

fn is_id_mapped<T>(id_map: &[IdMap<T>], id: T) -> bool
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
