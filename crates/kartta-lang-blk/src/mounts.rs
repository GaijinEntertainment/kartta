use std::collections::HashMap;
use std::iter;

use kartta_lang::path::{dir, forward_slashes, join, sole};
use kartta_lang::{RawExtraction, Repository};

use crate::resolve::indexed;
use crate::walk::{MOUNT_BLOCK_FORM, MOUNT_PARAMETER_FORM};

// constraint: `MAX_MOUNT_NAME_LEN` of the engine's `dag_basePath.h`
const MAX_MOUNT_NAME_LEN: usize = 64;

#[derive(Default)]
pub(crate) struct MountTable {
    mounts: HashMap<String, Vec<Mount>>,
}

struct Mount {
    stating_dir: String,
    dir: MountDir,
}

enum MountDir {
    InRepository(String),
    BelowAncestor(String),
    Outside,
}

impl MountTable {
    pub(crate) fn add(&mut self, path: &str, raw: &RawExtraction) {
        let stating_dir = dir(path);
        for (name, mount_dir) in stated_mounts(raw, stating_dir) {
            self.mounts.entry(name).or_default().push(Mount {
                stating_dir: stating_dir.to_owned(),
                dir: mount_dir,
            });
        }
    }

    // constraint: the engine reads a mount name up to the first `/` of the path and in its letter case
    pub(crate) fn resolve(
        &self,
        path: &str,
        including: &str,
        repository: &dyn Repository,
    ) -> Option<String> {
        let (name, rest) = path.strip_prefix('%')?.split_once('/')?;
        let mounts = self.mounts.get(name)?;
        let nearness = |mount: &Mount| shared_segments(&mount.stating_dir, including);
        let nearest = mounts.iter().map(nearness).max()?;
        let rest = forward_slashes(rest);

        let mut target: Option<String> = None;
        for mount in mounts.iter().filter(|mount| nearness(mount) == nearest) {
            let found = mount.place(&rest, repository)?;
            if target.as_ref().is_some_and(|target| *target != found) {
                return None;
            }
            target = Some(found);
        }

        target
    }
}

impl Mount {
    fn place(&self, rest: &str, repository: &dyn Repository) -> Option<String> {
        match &self.dir {
            MountDir::InRepository(mount_dir) => indexed(join(mount_dir, rest)?, repository),
            MountDir::BelowAncestor(tail) => {
                sole(ancestors(&self.stating_dir).filter_map(|ancestor| {
                    let mount_dir = join(ancestor, tail)?;
                    indexed(join(&mount_dir, rest)?, repository)
                }))
            }
            MountDir::Outside => None,
        }
    }
}

// constraint: a game or a tool reads the block form and an AOT compiler the parameter form, and each lets a later
// entry of a name replace an earlier one
fn stated_mounts(raw: &RawExtraction, stating_dir: &str) -> Vec<(String, MountDir)> {
    let mut from_blocks: Vec<(String, MountDir)> = Vec::new();
    let mut from_parameters: Vec<(String, MountDir)> = Vec::new();

    for directive in &raw.directives {
        let mount_dir = directive.names.first();
        let (mounts, mount_dir) = match directive.form {
            MOUNT_BLOCK_FORM => (
                &mut from_blocks,
                mount_dir.map(|path| below_working_dir(path)),
            ),
            MOUNT_PARAMETER_FORM => (
                &mut from_parameters,
                mount_dir.map(|path| relative_to(stating_dir, path)),
            ),
            _ => continue,
        };
        if let Some(name) = mount_name(&directive.spec) {
            mounts.retain(|(mapped, _)| mapped != name);
            if let Some(mount_dir) = mount_dir {
                mounts.push((name.to_owned(), mount_dir));
            }
        }
    }
    from_blocks.extend(from_parameters);

    from_blocks
}

// constraint: `dd_set_named_mount_path` drops one leading `%` and rejects a name of 64 bytes or more, or one with a
// byte other than an ASCII letter, a digit or `_`
fn mount_name(name: &str) -> Option<&str> {
    let name = name.strip_prefix('%').unwrap_or(name);
    let valid = name.len() < MAX_MOUNT_NAME_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');

    valid.then_some(name)
}

// constraint: a block's `forSource` is relative to the application's working directory, which no file states and
// which lies below an ancestor of the stating file, so a directory that climbs out of it names the rest below some
// ancestor, and one that does not climb lies inside it and names no repository directory
fn below_working_dir(path: &str) -> MountDir {
    let path = forward_slashes(path);
    if is_outside(&path) {
        return MountDir::Outside;
    }

    let mut components = Vec::new();
    let mut climbs = false;
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => climbs |= components.pop().is_none(),
            name => components.push(name),
        }
    }

    if climbs {
        MountDir::BelowAncestor(components.join("/"))
    } else {
        MountDir::Outside
    }
}

// constraint: an AOT compiler reads a mount parameter relative to the directory of its `--config` file
fn relative_to(stating_dir: &str, path: &str) -> MountDir {
    let path = forward_slashes(path);
    if is_outside(&path) {
        return MountDir::Outside;
    }

    join(stating_dir, &path).map_or(MountDir::Outside, MountDir::InRepository)
}

fn is_outside(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('%') || path.contains(':')
}

fn ancestors(dir: &str) -> impl Iterator<Item = &str> {
    let start = if dir == "." { "" } else { dir };

    iter::successors(Some(start), |dir| {
        (!dir.is_empty()).then(|| dir.rfind('/').map_or("", |slash| &dir[..slash]))
    })
}

fn shared_segments(dir: &str, path: &str) -> usize {
    if dir == "." {
        return 0;
    }

    dir.split('/')
        .zip(path.split('/'))
        .take_while(|(left, right)| left == right)
        .count()
}
