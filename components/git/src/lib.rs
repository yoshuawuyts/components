//! Self-contained Git repository operations for WASI agents.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::error_impl_error,
    clippy::same_length_and_capacity,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]

wit_bindgen::generate!({
    world: "git",
    path: "wit",
});

use exports::yoshuawuyts::git::repository::{
    Change, Commit, Difference, Entry, Error, FileMode, Guest, Reference, Signature,
};
use gix::bstr::ByteSlice;
use std::collections::{BTreeMap, BTreeSet};

const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 128;

/// Native entry point, also exported through the component's WIT interface.
#[derive(Debug)]
pub struct Component;

#[cfg(target_arch = "wasm32")]
export!(Component);

impl Guest for Component {
    fn init(path: String, branch: String) -> Result<(), Error> {
        let name = branch_name(&branch)?;
        validate_repository_path(&path)?;
        // Exclusive creation prevents accidentally reinitializing an existing repo.
        std::fs::create_dir(&path).map_err(repository_error)?;
        gix::create::into(
            &path,
            gix::create::Kind::Bare,
            gix::create::Options::default(),
        )
        .map_err(repository_error)?;
        std::fs::write(
            std::path::Path::new(&path).join("HEAD"),
            format!("ref: {name}\n"),
        )
        .map_err(repository_error)
    }
    fn resolve(path: String, revision: String) -> Result<String, Error> {
        let repo = open(&path)?;
        Ok(resolve(&repo, &revision)?.to_string())
    }
    fn references(path: String) -> Result<Vec<Reference>, Error> {
        let repo = open(&path)?;
        let platform = repo.references().map_err(repository_error)?;
        let mut out = Vec::new();
        for reference in platform.all().map_err(repository_error)? {
            let mut reference = reference.map_err(repository_error)?;
            let name = utf8(reference.name().as_bstr())?;
            let id = reference
                .peel_to_id()
                .map_err(repository_error)?
                .to_string();
            out.push(Reference { name, id });
            if out.len() > MAX_ENTRIES {
                return Err(Error::Unsupported("too many references".into()));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
    fn log(path: String, revision: String, limit: u32) -> Result<Vec<Commit>, Error> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::InvalidInput("log limit must be 1..=1000".into()));
        }
        let repo = open(&path)?;
        let mut id = commit_id(&repo, &revision)?;
        let mut out = Vec::new();
        for _ in 0..limit {
            let commit = repo.find_commit(id).map_err(repository_error)?;
            let decoded = commit.decode().map_err(repository_error)?;
            let parents: Vec<_> = decoded.parents().collect();
            out.push(Commit {
                id: id.to_string(),
                tree: decoded.tree().to_string(),
                parents: parents.iter().map(ToString::to_string).collect(),
                author: decode_signature(commit.author().map_err(repository_error)?)?,
                committer: decode_signature(commit.committer().map_err(repository_error)?)?,
                message: decoded.message.to_vec(),
            });
            let Some(parent) = parents.first() else {
                break;
            };
            id = *parent;
        }
        Ok(out)
    }
    fn list_tree(path: String, revision: String) -> Result<Vec<Entry>, Error> {
        let repo = open(&path)?;
        Ok(entries(&repo, tree_id(&repo, &revision)?)?
            .into_values()
            .collect())
    }
    fn read_file(path: String, revision: String, file: String) -> Result<Vec<u8>, Error> {
        validate_file_path(&file)?;
        let repo = open(&path)?;
        let tree = repo
            .find_tree(tree_id(&repo, &revision)?)
            .map_err(repository_error)?;
        let entry = tree
            .lookup_entry(file.split('/').map(str::as_bytes))
            .map_err(repository_error)?
            .ok_or_else(|| Error::InvalidInput(format!("file not found: {file}")))?;
        if entry.mode().is_tree() || entry.mode().is_commit() {
            return Err(Error::InvalidInput("path is not a blob".into()));
        }
        let blob = repo
            .find_blob(entry.object_id())
            .map_err(repository_error)?;
        if blob.data.len() > MAX_BLOB_BYTES {
            return Err(Error::Unsupported("blob exceeds 16 MiB".into()));
        }
        Ok(blob.data.clone())
    }
    fn diff(path: String, before: String, after: String) -> Result<Vec<Difference>, Error> {
        let repo = open(&path)?;
        let mut old = entries(&repo, tree_id(&repo, &before)?)?;
        let mut new = entries(&repo, tree_id(&repo, &after)?)?;
        let paths: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
        let mut out = Vec::new();
        for path in paths {
            let before = old.remove(&path);
            let after = new.remove(&path);
            if before.as_ref().map(|e| (&e.id, e.mode)) != after.as_ref().map(|e| (&e.id, e.mode)) {
                out.push(Difference {
                    path,
                    before,
                    after,
                });
            }
        }
        Ok(out)
    }
    fn create_branch(path: String, branch: String, revision: String) -> Result<(), Error> {
        let name = branch_name(&branch)?;
        let repo = open_bare(&path)?;
        if repo
            .try_find_reference(name.as_str())
            .map_err(repository_error)?
            .is_some()
        {
            return Err(Error::Conflict("branch already exists".into()));
        }
        let id = commit_id(&repo, &revision)?;
        update_branch(&repo, &name, id, None)
    }
    fn commit_files(
        path: String,
        branch: String,
        expected_parent: Option<String>,
        changes: Vec<Change>,
        author: Signature,
        message: String,
    ) -> Result<String, Error> {
        let name = branch_name(&branch)?;
        let signature = encode_signature(author)?;
        if message.trim().is_empty() || message.contains('\0') {
            return Err(Error::InvalidInput(
                "commit message must be nonempty and contain no NUL".into(),
            ));
        }
        validate_changes(&changes)?;
        let repo = open_bare(&path)?;
        let parent = expected_parent
            .map(|id| gix::ObjectId::from_hex(id.as_bytes()).map_err(invalid_input))
            .transpose()?;
        let current = repo
            .try_find_reference(name.as_str())
            .map_err(repository_error)?;
        if current
            .as_ref()
            .and_then(gix::Reference::try_id)
            .map(gix::Id::detach)
            != parent
            || current.as_ref().is_some_and(|r| r.try_id().is_none())
        {
            return Err(Error::Conflict(
                "branch changed; reread its current commit".into(),
            ));
        }
        let base = match parent {
            Some(id) => repo
                .find_commit(id)
                .map_err(repository_error)?
                .tree_id()
                .map_err(repository_error)?
                .detach(),
            None => gix::ObjectId::empty_tree(repo.object_hash()),
        };
        let base_tree = repo.find_tree(base).map_err(repository_error)?;
        for change in &changes {
            for (index, _) in change.path.match_indices('/') {
                let prefix = change
                    .path
                    .get(..index)
                    .ok_or_else(|| Error::InvalidInput("invalid path boundary".into()))?;
                if base_tree
                    .lookup_entry(prefix.split('/').map(str::as_bytes))
                    .map_err(repository_error)?
                    .is_some_and(|e| !e.mode().is_tree())
                {
                    return Err(Error::InvalidInput(format!(
                        "path crosses a file: {prefix}"
                    )));
                }
            }
            let entry = base_tree
                .lookup_entry(change.path.split('/').map(str::as_bytes))
                .map_err(repository_error)?;
            if entry.as_ref().is_some_and(|e| e.mode().is_tree()) {
                return Err(Error::InvalidInput(format!(
                    "cannot replace or remove directory: {}",
                    change.path
                )));
            }
            if change.contents.is_none() && entry.is_none() {
                return Err(Error::InvalidInput(format!(
                    "cannot remove missing file: {}",
                    change.path
                )));
            }
        }
        let mut editor = repo.edit_tree(base).map_err(repository_error)?;
        for change in changes {
            if let Some(data) = change.contents {
                let blob = repo.write_blob(&data).map_err(repository_error)?;
                let kind = match change.mode {
                    FileMode::Regular => gix::objs::tree::EntryKind::Blob,
                    FileMode::Executable => gix::objs::tree::EntryKind::BlobExecutable,
                    FileMode::Symlink => gix::objs::tree::EntryKind::Link,
                };
                editor
                    .upsert(&change.path, kind, blob)
                    .map_err(repository_error)?;
            } else {
                editor.remove_leaf(&change.path).map_err(repository_error)?;
            }
        }
        let tree = editor.write().map_err(repository_error)?.detach();
        if tree == base {
            return Err(Error::InvalidInput("changes do not alter the tree".into()));
        }
        let mut time = gix::date::parse::TimeBuf::default();
        let signature = signature.to_ref(&mut time);
        let commit = repo
            .new_commit_as(signature, signature, &message, tree, parent)
            .map_err(repository_error)?;
        update_branch(&repo, &name, commit.id, parent)?;
        Ok(commit.id.to_string())
    }
}

fn repository_error(error: impl std::fmt::Debug) -> Error {
    Error::Repository(format!("{error:#?}"))
}

fn invalid_input(error: impl std::fmt::Display) -> Error {
    Error::InvalidInput(error.to_string())
}

fn utf8(bytes: &[u8]) -> Result<String, Error> {
    std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
        Error::Unsupported("non-UTF-8 names are not supported by this interface".into())
    })
}

fn validate_repository_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || path.contains('\0') {
        return Err(Error::InvalidInput(
            "repository path must be nonempty and contain no NUL".into(),
        ));
    }
    Ok(())
}

fn open(path: &str) -> Result<gix::Repository, Error> {
    validate_repository_path(path)?;
    gix::open_opts(path, gix::open::Options::isolated().strict_config(true))
        .map_err(repository_error)
}

fn open_bare(path: &str) -> Result<gix::Repository, Error> {
    let repo = open(path)?;
    if !repo.is_bare() {
        return Err(Error::Unsupported(
            "writes require a bare repository; indexes and worktrees are not modified".into(),
        ));
    }
    Ok(repo)
}

fn branch_name(branch: &str) -> Result<String, Error> {
    if branch.is_empty() || branch.starts_with('-') || branch == "@" {
        return Err(Error::InvalidInput("invalid branch name".into()));
    }
    let name = format!("refs/heads/{branch}");
    gix::validate::reference::branch_name(name.as_bytes().as_bstr()).map_err(invalid_input)?;
    Ok(name)
}

fn validate_file_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || path.contains(['\0', '\\', ':']) {
        return Err(Error::InvalidInput(
            "file paths must be portable, relative Git paths".into(),
        ));
    }
    let mut depth = 0;
    for part in path.split('/') {
        depth += 1;
        if part.is_empty() || part == "." || part == ".." || part.eq_ignore_ascii_case(".git") {
            return Err(Error::InvalidInput(format!("invalid file path: {path}")));
        }
    }
    if depth > MAX_DEPTH {
        return Err(Error::InvalidInput(
            "file path exceeds 128 components".into(),
        ));
    }
    Ok(())
}

fn validate_changes(changes: &[Change]) -> Result<(), Error> {
    if changes.is_empty() || changes.len() > 1000 {
        return Err(Error::InvalidInput(
            "commit requires 1..=1000 changes".into(),
        ));
    }
    let mut paths = BTreeSet::new();
    let mut bytes = 0_usize;
    for change in changes {
        validate_file_path(&change.path)?;
        if !paths.insert(change.path.as_str()) {
            return Err(Error::InvalidInput(format!(
                "duplicate change: {}",
                change.path
            )));
        }
        bytes = bytes.saturating_add(change.contents.as_ref().map_or(0, Vec::len));
    }
    if bytes > MAX_BLOB_BYTES {
        return Err(Error::InvalidInput(
            "commit contents exceed 16 MiB in total".into(),
        ));
    }
    for path in &paths {
        for (index, _) in path.match_indices('/') {
            if path
                .get(..index)
                .is_some_and(|prefix| paths.contains(prefix))
            {
                return Err(Error::InvalidInput(
                    "changes contain overlapping paths".into(),
                ));
            }
        }
    }
    Ok(())
}

fn resolve(repo: &gix::Repository, revision: &str) -> Result<gix::ObjectId, Error> {
    if revision.is_empty() || revision.contains('\0') {
        return Err(Error::InvalidInput(
            "revision must be nonempty and contain no NUL".into(),
        ));
    }
    if revision.starts_with(':') {
        return Err(Error::Unsupported(
            "colon-leading revision expressions (including index lookups) are not supported".into(),
        ));
    }
    repo.rev_parse_single(revision.as_bytes().as_bstr())
        .map(gix::Id::detach)
        .map_err(repository_error)
}

fn commit_id(repo: &gix::Repository, revision: &str) -> Result<gix::ObjectId, Error> {
    repo.find_object(resolve(repo, revision)?)
        .map_err(repository_error)?
        .peel_to_kind(gix::objs::Kind::Commit)
        .map_err(repository_error)
        .map(|object| object.id)
}

fn tree_id(repo: &gix::Repository, revision: &str) -> Result<gix::ObjectId, Error> {
    repo.find_object(resolve(repo, revision)?)
        .map_err(repository_error)?
        .peel_to_kind(gix::objs::Kind::Tree)
        .map_err(repository_error)
        .map(|object| object.id)
}

fn entries(repo: &gix::Repository, root: gix::ObjectId) -> Result<BTreeMap<String, Entry>, Error> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(String::new(), root, 0_usize)];
    let mut visited = 0_usize;
    while let Some((prefix, id, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::Unsupported("tree exceeds 128 levels".into()));
        }
        let tree = repo.find_tree(id).map_err(repository_error)?;
        for entry in tree.iter() {
            let entry = entry.map_err(repository_error)?;
            visited += 1;
            if visited > MAX_ENTRIES {
                return Err(Error::Unsupported("tree exceeds 100000 entries".into()));
            }
            let filename = utf8(entry.filename())?;
            if filename.is_empty() || filename.contains('/') {
                return Err(Error::Repository("invalid tree entry name".into()));
            }
            let path = format!("{prefix}{filename}");
            if entry.mode().is_tree() {
                stack.push((format!("{path}/"), entry.object_id(), depth + 1));
            } else {
                let mode = u32::from(entry.mode().value());
                let record = Entry {
                    path: path.clone(),
                    id: entry.object_id().to_string(),
                    mode,
                };
                if out.insert(path, record).is_some() {
                    return Err(Error::Repository("duplicate tree entry".into()));
                }
            }
        }
    }
    Ok(out)
}

fn decode_signature(signature: gix::actor::SignatureRef<'_>) -> Result<Signature, Error> {
    let time = signature.time().map_err(repository_error)?;
    Ok(Signature {
        name: utf8(signature.name)?,
        email: utf8(signature.email)?,
        seconds: time.seconds,
        offset: time.offset,
    })
}

fn encode_signature(signature: Signature) -> Result<gix::actor::Signature, Error> {
    if signature.name.trim().is_empty()
        || signature.email.trim().is_empty()
        || signature.name.contains(['<', '>', '\n', '\r', '\0'])
        || signature.email.contains(['<', '>', '\n', '\r', '\0'])
        || signature.offset.unsigned_abs() > 23 * 3600 + 59 * 60
        || signature.offset % 60 != 0
    {
        return Err(Error::InvalidInput(
            "invalid author identity or timezone offset".into(),
        ));
    }
    Ok(gix::actor::Signature {
        name: signature.name.into(),
        email: signature.email.into(),
        time: gix::date::Time {
            seconds: signature.seconds,
            offset: signature.offset,
        },
    })
}

fn update_branch(
    repo: &gix::Repository,
    name: &str,
    id: gix::ObjectId,
    parent: Option<gix::ObjectId>,
) -> Result<(), Error> {
    use std::io::Write;
    // gix-tempfile's named locks require a process ID, which WASI lacks.
    // Follow Git's .lock/create-new/rename protocol without process metadata.
    let target = repo.git_dir().join(name);
    let lock_path = repo.git_dir().join(format!("{name}.lock"));
    let directory = target
        .parent()
        .ok_or_else(|| Error::InvalidInput("invalid reference path".into()))?;
    std::fs::create_dir_all(directory).map_err(repository_error)?;
    let mut lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Conflict("branch is locked by another writer".into())
            } else {
                repository_error(e)
            }
        })?;
    let result = (|| {
        let fresh = open(
            repo.git_dir()
                .to_str()
                .ok_or_else(|| Error::Unsupported("non-UTF-8 repository path".into()))?,
        )?;
        let reference = fresh.try_find_reference(name).map_err(repository_error)?;
        if reference
            .as_ref()
            .and_then(gix::Reference::try_id)
            .map(gix::Id::detach)
            != parent
            || reference.as_ref().is_some_and(|r| r.try_id().is_none())
        {
            return Err(Error::Conflict("branch changed while committing".into()));
        }
        writeln!(lock, "{id}").map_err(repository_error)?;
        lock.sync_all().map_err(repository_error)?;
        Ok(())
    })();
    drop(lock);
    if let Err(error) = result {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!(
                "{error:?}; failed to remove reference lock: {cleanup}"
            ))
        })?;
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&lock_path, &target) {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!(
                "{error}; failed to remove reference lock: {cleanup}"
            ))
        })?;
        return Err(repository_error(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
