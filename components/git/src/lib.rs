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
    BlameLine, BlameOptions, Change, ChangeKind, CommitRecord, Difference, Entry, Error,
    FileDifference, FileMode, FilePatch, FileStatus, Guest, MergeConflict, MergeResult,
    PatchOptions, RebaseConflict, RebaseResult, RebaseSuccess, Reference, Signature,
};
use gix::bstr::ByteSlice;
use std::collections::{BTreeMap, BTreeSet};

const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 128;
const MAX_HISTORY: usize = 1000;

mod patches;

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
    fn log(path: String, revision: String, limit: u32) -> Result<Vec<CommitRecord>, Error> {
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
            out.push(CommitRecord {
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
    fn diff_renames(
        path: String,
        before: String,
        after: String,
        max_files: u32,
    ) -> Result<Vec<FileDifference>, Error> {
        patches::diff_renames(&open(&path)?, &before, &after, max_files)
    }
    fn unified_diff(
        path: String,
        before: String,
        after: String,
        options: PatchOptions,
    ) -> Result<Vec<FilePatch>, Error> {
        patches::unified_diff(&open(&path)?, &before, &after, &options)
    }
    fn blame(
        path: String,
        revision: String,
        file: String,
        options: BlameOptions,
    ) -> Result<Vec<BlameLine>, Error> {
        patches::blame(&open(&path)?, &revision, &file, &options)
    }
    fn checkout(path: String, revision: String, force: bool) -> Result<(), Error> {
        let repo = open_worktree(&path)?;
        let target_branch = checkout_branch(&repo, &revision)?;
        let target = commit_id(&repo, &revision)?;
        let target_tree = repo
            .find_commit(target)
            .map_err(repository_error)?
            .tree_id()
            .map_err(repository_error)?
            .detach();
        let target_entries = entries(&repo, target_tree)?;

        let current_index = read_index(&repo)?;
        let current_entries = index_entries(&current_index)?;
        let statuses = worktree_status(&repo, &current_index)?;
        if !force
            && statuses.iter().any(|status| {
                status.staged.is_some()
                    || status.unstaged.is_some()
                    || status.untracked
                        && target_entries.contains_key(&status.path)
                        && !current_entries.contains_key(&status.path)
            })
        {
            return Err(Error::Conflict(
                "checkout would overwrite uncommitted changes".into(),
            ));
        }
        if !force {
            for file in target_entries.keys() {
                if current_entries.contains_key(file) {
                    continue;
                }
                let destination = match worktree_file_path(&repo, file) {
                    Ok(path) => path,
                    Err(Error::InvalidInput(_)) => {
                        return Err(Error::Conflict(format!(
                            "checkout path collides with an existing path: {file}"
                        )));
                    }
                    Err(error) => return Err(error),
                };
                if std::fs::symlink_metadata(destination).is_ok() {
                    return Err(Error::Conflict(format!(
                        "checkout would overwrite untracked path: {file}"
                    )));
                }
            }
        }

        for (path, entry) in &target_entries {
            if entry.mode == 0o160_000 {
                return Err(Error::Unsupported(format!(
                    "checkout of submodule entry is not supported: {path}"
                )));
            }
            validate_checkout_entry(&repo, entry)?;
        }

        for old_path in current_entries.keys() {
            if !target_entries.contains_key(old_path) {
                remove_worktree_path(&repo, old_path, force)?;
            }
        }
        for entry in target_entries.values() {
            write_worktree_entry(&repo, entry, force)?;
        }

        let mut index = repo
            .index_from_tree(&target_tree)
            .map_err(repository_error)?;
        write_index_atomic(&repo, &mut index)?;
        write_head_atomic(
            &repo,
            target_branch
                .as_deref()
                .map_or_else(
                    || format!("{target}\n"),
                    |branch| format!("ref: refs/heads/{branch}\n"),
                )
                .as_bytes(),
        )?;
        Ok(())
    }
    fn status(path: String) -> Result<Vec<FileStatus>, Error> {
        let repo = open_worktree(&path)?;
        let index = read_index(&repo)?;
        worktree_status(&repo, &index)
    }
    fn add(path: String, paths: Vec<String>) -> Result<(), Error> {
        let repo = open_worktree(&path)?;
        let mut index = read_index(&repo)?;
        let mut entries = index_entries(&index)?;
        let mut seen = BTreeSet::new();
        if paths.is_empty() || paths.len() > MAX_ENTRIES {
            return Err(Error::InvalidInput("add requires 1..=100000 paths".into()));
        }
        for path in paths {
            validate_file_path(&path)?;
            if !seen.insert(path.clone()) {
                return Err(Error::InvalidInput(format!("duplicate path: {path}")));
            }
            let file = worktree_file_path(&repo, &path)?;
            let metadata = std::fs::symlink_metadata(&file).map_err(repository_error)?;
            if metadata.is_dir() {
                return Err(Error::Unsupported(format!(
                    "adding directories recursively is not supported: {path}"
                )));
            }
            let (data, mode) = if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&file).map_err(repository_error)?;
                let target = target.to_str().ok_or_else(|| {
                    Error::Unsupported("non-UTF-8 symlink targets are not supported".into())
                })?;
                (target.as_bytes().to_vec(), FileMode::Symlink)
            } else if metadata.is_file() {
                let data = std::fs::read(&file).map_err(repository_error)?;
                if data.len() > MAX_BLOB_BYTES {
                    return Err(Error::Unsupported("file exceeds 16 MiB".into()));
                }
                (data, worktree_file_mode(&metadata))
            } else {
                return Err(Error::Unsupported(format!(
                    "adding this file type is not supported: {path}"
                )));
            };
            let id = repo.write_blob(&data).map_err(repository_error)?;
            entries.insert(
                path.clone(),
                Entry {
                    path,
                    id: id.to_string(),
                    mode: file_mode_value(mode),
                },
            );
        }
        let tree = write_tree(&repo, &entries)?;
        index = repo.index_from_tree(&tree).map_err(repository_error)?;
        write_index_atomic(&repo, &mut index)
    }
    fn remove(path: String, paths: Vec<String>) -> Result<(), Error> {
        let repo = open_worktree(&path)?;
        let mut index = read_index(&repo)?;
        let mut entries = index_entries(&index)?;
        let paths = validate_paths(paths, "remove")?;
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let Some(entry) = entries.remove(&path) else {
                return Err(Error::InvalidInput(format!(
                    "cannot remove untracked path: {path}"
                )));
            };
            if worktree_change(&repo, &path, &entry)?.is_some() {
                return Err(Error::Conflict(format!(
                    "cannot remove modified working-tree path: {path}"
                )));
            }
            let file = worktree_file_path(&repo, &path)?;
            match std::fs::symlink_metadata(&file) {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                    files.push(file);
                }
                Ok(_) => {
                    return Err(Error::Unsupported(format!(
                        "removing directories and special files is not supported: {path}"
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(repository_error(error)),
            }
        }
        let tree = write_tree(&repo, &entries)?;
        index = repo.index_from_tree(&tree).map_err(repository_error)?;
        write_index_atomic(&repo, &mut index)?;
        for file in files {
            std::fs::remove_file(file).map_err(repository_error)?;
        }
        Ok(())
    }
    fn reset(path: String, paths: Vec<String>) -> Result<(), Error> {
        let repo = open_worktree(&path)?;
        let mut index = read_index(&repo)?;
        let mut indexed = index_entries(&index)?;
        let paths = validate_paths(paths, "reset")?;
        let head_tree = repo
            .head_tree_id_or_empty()
            .map_err(repository_error)?
            .detach();
        let head_entries = entries(&repo, head_tree)?;
        for path in paths {
            indexed.remove(&path);
            if let Some(head_entry) = head_entries.get(&path) {
                indexed.insert(path, head_entry.clone());
            }
        }
        let tree = write_tree(&repo, &indexed)?;
        index = repo.index_from_tree(&tree).map_err(repository_error)?;
        write_index_atomic(&repo, &mut index)
    }
    fn commit(
        path: String,
        expected_parent: Option<String>,
        author: Signature,
        message: String,
    ) -> Result<String, Error> {
        let signature = encode_signature(author)?;
        if message.trim().is_empty() || message.contains('\0') {
            return Err(Error::InvalidInput(
                "commit message must be nonempty and contain no NUL".into(),
            ));
        }
        let repo = open_worktree(&path)?;
        let head = repo
            .head_name()
            .map_err(repository_error)?
            .ok_or_else(|| Error::Unsupported("commit requires a checked-out branch".into()))?;
        let branch = utf8(head.as_bstr())?
            .strip_prefix("refs/heads/")
            .ok_or_else(|| Error::Unsupported("HEAD does not point to a local branch".into()))?
            .to_owned();
        let name = branch_name(&branch)?;
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
        let index = read_index(&repo)?;
        let indexed = index_entries(&index)?;
        let tree = write_tree(&repo, &indexed)?;
        if let Some(parent) = parent {
            let parent_tree = repo
                .find_commit(parent)
                .map_err(repository_error)?
                .tree_id()
                .map_err(repository_error)?
                .detach();
            if tree == parent_tree {
                return Err(Error::InvalidInput(
                    "index does not contain changes to commit".into(),
                ));
            }
        }
        let mut time = gix::date::parse::TimeBuf::default();
        let signature = signature.to_ref(&mut time);
        let commit = repo
            .new_commit_as(signature, signature, &message, tree, parent)
            .map_err(repository_error)?;
        update_branch(&repo, &name, commit.id, parent)?;
        Ok(commit.id.to_string())
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
    fn merge_branch(
        path: String,
        target_branch: String,
        source_branch: String,
        expected_target_tip: String,
        expected_source_tip: String,
        committer: Signature,
        message: String,
    ) -> Result<MergeResult, Error> {
        let target_name = branch_name(&target_branch)?;
        let source_name = branch_name(&source_branch)?;
        let committer = encode_signature(committer)?;
        validate_commit_message(&message)?;
        let repo = open_bare(&path)?;
        let target_tip = branch_tip(&repo, &target_name)?;
        let source_tip = branch_tip(&repo, &source_name)?;
        let expected_target = full_object_id(&expected_target_tip)?;
        let expected_source = full_object_id(&expected_source_tip)?;
        if target_tip != expected_target || source_tip != expected_source {
            return Err(Error::Conflict(
                "branch tip changed; reread both branch tips before retrying".into(),
            ));
        }
        ensure_bounded_history(&repo, target_tip)?;
        ensure_bounded_history(&repo, source_tip)?;

        if is_ancestor(&repo, target_tip, source_tip)? {
            if target_tip == source_tip {
                return Ok(MergeResult::UpToDate(target_tip.to_string()));
            }
            update_branch(&repo, &target_name, source_tip, Some(target_tip))?;
            return Ok(MergeResult::FastForward(source_tip.to_string()));
        }
        if is_ancestor(&repo, source_tip, target_tip)? {
            return Ok(MergeResult::UpToDate(target_tip.to_string()));
        }

        let target_tree = commit_tree(&repo, target_tip)?;
        let source_tree = commit_tree(&repo, source_tip)?;
        ensure_merge_supported(&repo, &[target_tree, source_tree])?;
        let options = repo.tree_merge_options().map_err(repository_error)?;
        let mut outcome = repo
            .merge_commits(
                target_tip,
                source_tip,
                gix::merge::blob::builtin_driver::text::Labels::default(),
                options.into(),
            )
            .map_err(repository_error)?;
        let conflicts = unresolved_conflicts(outcome.tree_merge.conflicts.as_slice())?;
        if !conflicts.is_empty() {
            return Ok(MergeResult::Conflicts(conflicts));
        }
        let tree = outcome
            .tree_merge
            .tree
            .write()
            .map_err(repository_error)?
            .detach();
        ensure_merge_supported(&repo, &[tree])?;
        let mut time = gix::date::parse::TimeBuf::default();
        let committer_ref = committer.to_ref(&mut time);
        let commit = repo
            .new_commit_as(
                committer_ref,
                committer_ref,
                message,
                tree,
                [target_tip, source_tip],
            )
            .map_err(repository_error)?;
        update_branch(&repo, &target_name, commit.id, Some(target_tip))?;
        Ok(MergeResult::Merged(commit.id.to_string()))
    }
    fn rebase_branch(
        path: String,
        branch: String,
        expected_tip: String,
        new_base: String,
        committer: Signature,
    ) -> Result<RebaseResult, Error> {
        let name = branch_name(&branch)?;
        let committer = encode_signature(committer)?;
        let repo = open_bare(&path)?;
        let tip = branch_tip(&repo, &name)?;
        let expected_tip = full_object_id(&expected_tip)?;
        let base = full_object_id(&new_base)?;
        if tip != expected_tip {
            return Err(Error::Conflict(
                "branch tip changed; reread it before retrying".into(),
            ));
        }
        repo.find_commit(base).map_err(repository_error)?;
        ensure_bounded_history(&repo, tip)?;
        ensure_bounded_history(&repo, base)?;
        let merge_base = repo
            .merge_base(tip, base)
            .map(gix::Id::detach)
            .map_err(repository_error)?;

        let mut replay = Vec::new();
        let mut current = tip;
        while current != merge_base {
            if replay.len() == 1000 {
                return Err(Error::Unsupported("rebase exceeds 1000 commits".into()));
            }
            let commit = repo.find_commit(current).map_err(repository_error)?;
            let parents: Vec<_> = commit.parent_ids().map(gix::Id::detach).collect();
            if parents.len() > 1 {
                return Err(Error::Unsupported(
                    "rebasing merge commits is not supported".into(),
                ));
            }
            let Some(parent) = parents.first().copied() else {
                return Err(Error::Unsupported(
                    "merge base is not on the branch's first-parent history".into(),
                ));
            };
            replay.push(current);
            current = parent;
        }
        replay.reverse();

        let mut rebased_tip = base;
        let mut rewritten = Vec::with_capacity(replay.len());
        for original_id in replay {
            let original = repo.find_commit(original_id).map_err(repository_error)?;
            let parent = original
                .parent_ids()
                .next()
                .map(gix::Id::detach)
                .ok_or_else(|| Error::Repository("rebase commit has no parent".into()))?;
            let base_tree = commit_tree(&repo, parent)?;
            let our_tree = commit_tree(&repo, rebased_tip)?;
            let their_tree = commit_tree(&repo, original_id)?;
            ensure_merge_supported(&repo, &[base_tree, our_tree, their_tree])?;
            let options = repo.tree_merge_options().map_err(repository_error)?;
            let mut outcome = repo
                .merge_trees(
                    base_tree,
                    our_tree,
                    their_tree,
                    gix::merge::blob::builtin_driver::text::Labels::default(),
                    options,
                )
                .map_err(repository_error)?;
            let conflicts = unresolved_conflicts(outcome.conflicts.as_slice())?;
            if !conflicts.is_empty() {
                return Ok(RebaseResult::Conflicts(RebaseConflict {
                    commit: original_id.to_string(),
                    paths: conflicts
                        .into_iter()
                        .map(|conflict| conflict.path)
                        .collect(),
                }));
            }
            let tree = outcome.tree.write().map_err(repository_error)?.detach();
            ensure_merge_supported(&repo, &[tree])?;
            let author = original.author().map_err(repository_error)?;
            let message = original.message_raw().map_err(repository_error)?.to_owned();
            let mut time = gix::date::parse::TimeBuf::default();
            let committer_ref = committer.to_ref(&mut time);
            let commit = repo
                .write_object(gix::objs::Commit {
                    message,
                    tree,
                    author: author.into(),
                    committer: committer_ref.into(),
                    encoding: None,
                    parents: vec![rebased_tip].into(),
                    extra_headers: Vec::default(),
                })
                .map_err(repository_error)?;
            rebased_tip = commit.detach();
            rewritten.push(rebased_tip.to_string());
        }

        if rebased_tip != tip {
            update_branch(&repo, &name, rebased_tip, Some(tip))?;
        }
        Ok(RebaseResult::Rebased(RebaseSuccess {
            tip: rebased_tip.to_string(),
            commits: rewritten,
        }))
    }
}

fn repository_error(error: impl std::fmt::Debug) -> Error {
    Error::Repository(format!("{error:#?}"))
}

fn open_worktree(path: &str) -> Result<gix::Repository, Error> {
    let repo = open(path)?;
    if repo.is_bare() || repo.workdir().is_none() {
        return Err(Error::Unsupported(
            "operation requires a working repository".into(),
        ));
    }
    Ok(repo)
}

fn checkout_branch(repo: &gix::Repository, revision: &str) -> Result<Option<String>, Error> {
    if revision == "HEAD" {
        return repo
            .head_name()
            .map_err(repository_error)?
            .map(|name| utf8(name.as_bstr()))
            .transpose()
            .map(|name| name.and_then(|name| name.strip_prefix("refs/heads/").map(str::to_owned)));
    }
    let candidate = revision.strip_prefix("refs/heads/").unwrap_or(revision);
    let Ok(name) = branch_name(candidate) else {
        return Ok(None);
    };
    Ok(repo
        .try_find_reference(name.as_str())
        .map_err(repository_error)?
        .map(|_| candidate.to_owned()))
}

fn read_index(repo: &gix::Repository) -> Result<gix::index::File, Error> {
    let path = repo.index_path();
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(gix::index::File::from_state(
                gix::index::State::new(repo.object_hash()),
                path,
            ));
        }
        Err(error) => return Err(repository_error(error)),
    };
    if data.len() > MAX_BLOB_BYTES {
        return Err(Error::Unsupported("index exceeds 16 MiB".into()));
    }
    let modified = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(std::time::UNIX_EPOCH);
    let timestamp = filetime::FileTime::from_system_time(modified);
    let (state, _) = gix::index::State::from_bytes(
        &data,
        timestamp,
        repo.object_hash(),
        gix::index::decode::Options {
            thread_limit: Some(1),
            ..Default::default()
        },
    )
    .map_err(repository_error)?;
    Ok(gix::index::File::from_state(state, path))
}

fn write_index_atomic(repo: &gix::Repository, index: &mut gix::index::File) -> Result<(), Error> {
    use std::io::Write;

    let path = repo.index_path();
    let lock_path = path.with_file_name("index.lock");
    let mut lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Conflict("index is locked by another writer".into())
            } else {
                repository_error(error)
            }
        })?;
    index.remove_tree();
    let result = (|| {
        index
            .write_to(&mut lock, gix::index::write::Options::default())
            .map_err(repository_error)?;
        lock.flush().map_err(repository_error)?;
        lock.sync_all().map_err(repository_error)
    })();
    drop(lock);
    if let Err(error) = result {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!("{error:?}; failed to remove index lock: {cleanup}"))
        })?;
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&lock_path, &path) {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!("{error}; failed to remove index lock: {cleanup}"))
        })?;
        return Err(repository_error(error));
    }
    Ok(())
}

fn index_entries(index: &gix::index::File) -> Result<BTreeMap<String, Entry>, Error> {
    let mut entries = BTreeMap::new();
    for entry in index.entries() {
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(Error::Unsupported(
                "unmerged index entries are not supported".into(),
            ));
        }
        if entry.flags.intersects(
            gix::index::entry::Flags::ASSUME_VALID
                | gix::index::entry::Flags::INTENT_TO_ADD
                | gix::index::entry::Flags::SKIP_WORKTREE
                | gix::index::entry::Flags::UPDATE_IN_BASE
                | gix::index::entry::Flags::STRIP_NAME,
        ) {
            return Err(Error::Unsupported(
                "special and split-index entries are not supported".into(),
            ));
        }
        let path = utf8(entry.path(index))?;
        validate_file_path(&path)?;
        let value = Entry {
            path: path.clone(),
            id: entry.id.to_string(),
            mode: entry.mode.bits(),
        };
        if entries.insert(path, value).is_some() {
            return Err(Error::Repository("duplicate index entry".into()));
        }
    }
    Ok(entries)
}

fn write_tree(
    repo: &gix::Repository,
    entries: &BTreeMap<String, Entry>,
) -> Result<gix::ObjectId, Error> {
    if entries.len() > MAX_ENTRIES {
        return Err(Error::Unsupported("index exceeds 100000 entries".into()));
    }
    let base = gix::ObjectId::empty_tree(repo.object_hash());
    let mut editor = repo.edit_tree(base).map_err(repository_error)?;
    for entry in entries.values() {
        validate_file_path(&entry.path)?;
        let id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(invalid_input)?;
        let kind = match entry.mode {
            0o100_644 => gix::objs::tree::EntryKind::Blob,
            0o100_755 => gix::objs::tree::EntryKind::BlobExecutable,
            0o120_000 => gix::objs::tree::EntryKind::Link,
            0o160_000 => gix::objs::tree::EntryKind::Commit,
            _ => {
                return Err(Error::Unsupported(format!(
                    "unsupported index mode {:o} for {}",
                    entry.mode, entry.path
                )));
            }
        };
        editor
            .upsert(&entry.path, kind, id)
            .map_err(repository_error)?;
    }
    Ok(editor.write().map_err(repository_error)?.detach())
}

fn validate_paths(paths: Vec<String>, operation: &str) -> Result<Vec<String>, Error> {
    if paths.is_empty() || paths.len() > MAX_ENTRIES {
        return Err(Error::InvalidInput(format!(
            "{operation} requires 1..=100000 paths"
        )));
    }
    let mut unique = BTreeSet::new();
    for path in &paths {
        validate_file_path(path)?;
        if !unique.insert(path.as_str()) {
            return Err(Error::InvalidInput(format!("duplicate path: {path}")));
        }
    }
    Ok(paths)
}

fn worktree_file_path(repo: &gix::Repository, path: &str) -> Result<std::path::PathBuf, Error> {
    validate_file_path(path)?;
    let root = repo
        .workdir()
        .ok_or_else(|| Error::Unsupported("operation requires a working repository".into()))?;
    let mut current = root.to_owned();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        current.push(part);
        if parts.peek().is_some() {
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(Error::InvalidInput(format!(
                        "path crosses a non-directory: {path}"
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(repository_error(error)),
            }
        }
    }
    Ok(current)
}

fn remove_worktree_path(repo: &gix::Repository, path: &str, force: bool) -> Result<(), Error> {
    let file = worktree_file_path(repo, path)?;
    match std::fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.is_dir() => {
            if force {
                std::fs::remove_dir_all(file).map_err(repository_error)?;
            } else {
                std::fs::remove_dir(file).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                        Error::Conflict(format!("checkout would overwrite untracked path: {path}"))
                    } else {
                        repository_error(error)
                    }
                })?;
            }
        }
        Ok(_) => std::fs::remove_file(file).map_err(repository_error)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(repository_error(error)),
    }
    Ok(())
}

fn write_worktree_entry(repo: &gix::Repository, entry: &Entry, force: bool) -> Result<(), Error> {
    validate_file_path(&entry.path)?;
    let root = repo
        .workdir()
        .ok_or_else(|| Error::Unsupported("checkout requires a working repository".into()))?;
    let relative = std::path::Path::new(&entry.path);
    let file = root.join(relative);
    let mut parent = root.to_owned();
    for component in relative
        .parent()
        .into_iter()
        .flat_map(std::path::Path::components)
    {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) if force => {
                std::fs::remove_file(&parent).map_err(repository_error)?;
                std::fs::create_dir(&parent).map_err(repository_error)?;
            }
            Ok(_) => {
                return Err(Error::Conflict(format!(
                    "checkout path crosses a non-directory: {}",
                    entry.path
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent).map_err(repository_error)?;
            }
            Err(error) => return Err(repository_error(error)),
        }
    }

    if let Ok(metadata) = std::fs::symlink_metadata(&file) {
        if metadata.is_dir() {
            if force {
                std::fs::remove_dir_all(&file).map_err(repository_error)?;
            } else {
                std::fs::remove_dir(&file).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                        Error::Conflict(format!(
                            "checkout would overwrite untracked path: {}",
                            entry.path
                        ))
                    } else {
                        repository_error(error)
                    }
                })?;
            }
        } else {
            std::fs::remove_file(&file).map_err(repository_error)?;
        }
    }

    let id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(invalid_input)?;
    let object = repo.find_object(id).map_err(repository_error)?;
    if object.kind != gix::objs::Kind::Blob {
        return Err(Error::Repository(format!(
            "expected blob for checkout path {}",
            entry.path
        )));
    }
    if object.data.len() > MAX_BLOB_BYTES {
        return Err(Error::Unsupported(format!(
            "checkout blob exceeds 16 MiB: {}",
            entry.path
        )));
    }
    match entry.mode {
        0o100_644 | 0o100_755 => {
            std::fs::write(&file, &object.data).map_err(repository_error)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if entry.mode == 0o100_755 {
                    0o755
                } else {
                    0o644
                };
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode))
                    .map_err(repository_error)?;
            }
        }
        0o120_000 => {
            let target = std::str::from_utf8(&object.data).map_err(|_| {
                Error::Unsupported(format!(
                    "non-UTF-8 symlink targets are not supported: {}",
                    entry.path
                ))
            })?;
            gix::fs::symlink::create(std::path::Path::new(target), &file)
                .map_err(repository_error)?;
        }
        mode => {
            return Err(Error::Unsupported(format!(
                "unsupported checkout mode {mode:o} for {}",
                entry.path
            )));
        }
    }
    Ok(())
}

fn validate_checkout_entry(repo: &gix::Repository, entry: &Entry) -> Result<(), Error> {
    if !matches!(entry.mode, 0o100_644 | 0o100_755 | 0o120_000) {
        return Err(Error::Unsupported(format!(
            "unsupported checkout mode {:o} for {}",
            entry.mode, entry.path
        )));
    }
    let id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(invalid_input)?;
    let object = repo.find_object(id).map_err(repository_error)?;
    if object.kind != gix::objs::Kind::Blob {
        return Err(Error::Repository(format!(
            "expected blob for checkout path {}",
            entry.path
        )));
    }
    if object.data.len() > MAX_BLOB_BYTES {
        return Err(Error::Unsupported(format!(
            "checkout blob exceeds 16 MiB: {}",
            entry.path
        )));
    }
    if entry.mode == 0o120_000 && std::str::from_utf8(&object.data).is_err() {
        return Err(Error::Unsupported(format!(
            "non-UTF-8 symlink targets are not supported: {}",
            entry.path
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn worktree_file_mode(metadata: &std::fs::Metadata) -> FileMode {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o111 != 0 {
        FileMode::Executable
    } else {
        FileMode::Regular
    }
}

#[cfg(not(unix))]
fn worktree_file_mode(_: &std::fs::Metadata) -> FileMode {
    FileMode::Regular
}

fn file_mode_value(mode: FileMode) -> u32 {
    match mode {
        FileMode::Regular => 0o100_644,
        FileMode::Executable => 0o100_755,
        FileMode::Symlink => 0o120_000,
    }
}

fn status_kind(before: Option<&Entry>, after: Option<&Entry>) -> Option<ChangeKind> {
    match (before, after) {
        (None, Some(_)) => Some(ChangeKind::Added),
        (Some(_), None) => Some(ChangeKind::Removed),
        (Some(before), Some(after)) if before.mode != after.mode => Some(ChangeKind::TypeChanged),
        (Some(before), Some(after)) if before.id != after.id => Some(ChangeKind::Modified),
        (None, None) | (Some(_), Some(_)) => None,
    }
}

fn worktree_status(
    repo: &gix::Repository,
    index: &gix::index::File,
) -> Result<Vec<FileStatus>, Error> {
    let indexed = index_entries(index)?;
    let head_tree = repo
        .head_tree_id_or_empty()
        .map_err(repository_error)?
        .detach();
    let head = entries(repo, head_tree)?;
    let paths: BTreeSet<_> = head.keys().chain(indexed.keys()).cloned().collect();
    let mut statuses = BTreeMap::new();
    for path in paths {
        let staged = status_kind(head.get(&path), indexed.get(&path));
        if staged.is_some() {
            statuses.insert(
                path.clone(),
                FileStatus {
                    path,
                    staged,
                    unstaged: None,
                    untracked: false,
                },
            );
        }
    }
    for (path, entry) in &indexed {
        if let Some(kind) = worktree_change(repo, path, entry)? {
            statuses
                .entry(path.clone())
                .and_modify(|status: &mut FileStatus| status.unstaged = Some(kind))
                .or_insert(FileStatus {
                    path: path.clone(),
                    staged: None,
                    unstaged: Some(kind),
                    untracked: false,
                });
        }
    }

    let mut options = repo.dirwalk_options().map_err(repository_error)?;
    options = options.emit_untracked(gix::dir::walk::EmissionMode::Matching);
    let mut collected = gix::dir::walk::delegate::Collect::default();
    repo.dirwalk(
        index,
        std::iter::empty::<&[u8]>(),
        &std::sync::atomic::AtomicBool::new(false),
        options,
        &mut collected,
    )
    .map_err(repository_error)?;
    for (entry, _) in collected.into_entries_by_path() {
        if entry.status == gix::dir::entry::Status::Untracked {
            let path = utf8(&entry.rela_path)?;
            statuses.insert(
                path.clone(),
                FileStatus {
                    path,
                    staged: None,
                    unstaged: None,
                    untracked: true,
                },
            );
        }
    }
    Ok(statuses.into_values().collect())
}

fn worktree_change(
    repo: &gix::Repository,
    path: &str,
    entry: &Entry,
) -> Result<Option<ChangeKind>, Error> {
    let file = worktree_file_path(repo, path)?;
    let metadata = match std::fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(ChangeKind::Removed));
        }
        Err(error) => return Err(repository_error(error)),
    };
    if metadata.is_dir() {
        return Ok(Some(ChangeKind::TypeChanged));
    }
    let (id, mode) = if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(&file).map_err(repository_error)?;
        let target = target.to_str().ok_or_else(|| {
            Error::Unsupported("non-UTF-8 symlink targets are not supported".into())
        })?;
        (
            gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, target.as_bytes())
                .map_err(repository_error)?,
            0o120_000,
        )
    } else if metadata.is_file() {
        let mut file = std::fs::File::open(&file).map_err(repository_error)?;
        let id = gix::objs::compute_stream_hash(
            repo.object_hash(),
            gix::objs::Kind::Blob,
            &mut file,
            metadata.len(),
            &mut gix::progress::Discard,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(repository_error)?;
        (id, file_mode_value(worktree_file_mode(&metadata)))
    } else {
        return Ok(Some(ChangeKind::TypeChanged));
    };
    if (entry.mode == 0o120_000) != (mode == 0o120_000)
        || entry.mode == 0o160_000
        || mode == 0o160_000
    {
        return Ok(Some(ChangeKind::TypeChanged));
    }
    let entry_id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(invalid_input)?;
    if entry_id != id || entry.mode != mode {
        return Ok(Some(ChangeKind::Modified));
    }
    Ok(None)
}

fn write_head_atomic(repo: &gix::Repository, data: &[u8]) -> Result<(), Error> {
    use std::io::Write;

    let target = repo.git_dir().join("HEAD");
    let lock_path = repo.git_dir().join("HEAD.lock");
    let mut lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Conflict("HEAD is locked by another writer".into())
            } else {
                repository_error(error)
            }
        })?;
    let result = (|| {
        lock.write_all(data).map_err(repository_error)?;
        lock.sync_all().map_err(repository_error)
    })();
    drop(lock);
    if let Err(error) = result {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!("{error:?}; failed to remove HEAD lock: {cleanup}"))
        })?;
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&lock_path, &target) {
        std::fs::remove_file(&lock_path).map_err(|cleanup| {
            Error::Repository(format!("{error}; failed to remove HEAD lock: {cleanup}"))
        })?;
        return Err(repository_error(error));
    }
    Ok(())
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

fn validate_commit_message(message: &str) -> Result<(), Error> {
    if message.trim().is_empty() || message.contains('\0') {
        return Err(Error::InvalidInput(
            "commit message must be nonempty and contain no NUL".into(),
        ));
    }
    Ok(())
}

fn full_object_id(value: &str) -> Result<gix::ObjectId, Error> {
    if value.len() != 40 {
        return Err(Error::InvalidInput(
            "expected a full 40-character SHA-1 object identifier".into(),
        ));
    }
    gix::ObjectId::from_hex(value.as_bytes()).map_err(invalid_input)
}

fn branch_tip(repo: &gix::Repository, name: &str) -> Result<gix::ObjectId, Error> {
    let reference = repo
        .try_find_reference(name)
        .map_err(repository_error)?
        .ok_or_else(|| Error::InvalidInput(format!("branch does not exist: {name}")))?;
    let id = reference
        .try_id()
        .map(gix::Id::detach)
        .ok_or_else(|| Error::Unsupported(format!("branch is not a direct reference: {name}")))?;
    let commit = repo.find_commit(id).map_err(repository_error)?;
    Ok(commit.id)
}

fn commit_tree(repo: &gix::Repository, id: gix::ObjectId) -> Result<gix::ObjectId, Error> {
    repo.find_commit(id)
        .map_err(repository_error)?
        .tree_id()
        .map(gix::Id::detach)
        .map_err(repository_error)
}

fn bounded_ancestors(
    repo: &gix::Repository,
    tip: gix::ObjectId,
) -> Result<BTreeSet<gix::ObjectId>, Error> {
    let mut pending = vec![tip];
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        if visited.len() > MAX_HISTORY {
            return Err(Error::Unsupported(
                "branch history exceeds the 1000-commit work limit".into(),
            ));
        }
        let commit = repo.find_commit(id).map_err(repository_error)?;
        pending.extend(commit.parent_ids().map(gix::Id::detach));
    }
    Ok(visited)
}

fn ensure_bounded_history(repo: &gix::Repository, tip: gix::ObjectId) -> Result<(), Error> {
    bounded_ancestors(repo, tip).map(|_| ())
}

fn is_ancestor(
    repo: &gix::Repository,
    ancestor: gix::ObjectId,
    descendant: gix::ObjectId,
) -> Result<bool, Error> {
    Ok(bounded_ancestors(repo, descendant)?.contains(&ancestor))
}

fn ensure_merge_supported(repo: &gix::Repository, trees: &[gix::ObjectId]) -> Result<(), Error> {
    for tree in trees {
        let entries = entries(repo, *tree)?;
        let mut total_bytes = 0_usize;
        for entry in entries.values() {
            if entry.path == ".gitattributes" || entry.path.ends_with("/.gitattributes") {
                return Err(Error::Unsupported(
                    "merge attributes and filters are not supported".into(),
                ));
            }
            if entry.mode == 0o160_000 {
                return Err(Error::Unsupported(
                    "merging submodule entries is not supported".into(),
                ));
            }
            let id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(invalid_input)?;
            let blob = repo.find_blob(id).map_err(repository_error)?;
            total_bytes = total_bytes.saturating_add(blob.data.len());
            if blob.data.len() > MAX_BLOB_BYTES || total_bytes > MAX_BLOB_BYTES {
                return Err(Error::Unsupported(
                    "merge tree contents exceed the 16 MiB limit".into(),
                ));
            }
        }
    }

    let config = repo.config_snapshot();
    if config.string("core.attributesFile").is_some() {
        return Err(Error::Unsupported(
            "configured attribute files are not supported for merge or rebase".into(),
        ));
    }
    match std::fs::symlink_metadata(repo.git_dir().join("info/attributes")) {
        Ok(_) => {
            return Err(Error::Unsupported(
                "repository attribute files are not supported for merge or rebase".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(repository_error(error)),
    }
    for section in config
        .plumbing()
        .sections_by_name("merge")
        .into_iter()
        .flatten()
    {
        if section.header().subsection_name().is_some() && section.value("driver").is_some() {
            return Err(Error::Unsupported(
                "external merge drivers are not supported".into(),
            ));
        }
    }
    for section in config
        .plumbing()
        .sections_by_name("filter")
        .into_iter()
        .flatten()
    {
        if ["clean", "smudge", "process"]
            .iter()
            .any(|key| section.value(key).is_some())
        {
            return Err(Error::Unsupported(
                "external filters are not supported".into(),
            ));
        }
    }
    Ok(())
}

fn unresolved_conflicts(
    conflicts: &[gix::merge::tree::Conflict],
) -> Result<Vec<MergeConflict>, Error> {
    let mut paths = BTreeSet::new();
    for conflict in conflicts {
        if conflict.is_unresolved(gix::merge::tree::TreatAsUnresolved::default()) {
            let (ours, theirs) = conflict.changes_in_resolution();
            paths.insert(utf8(ours.location())?);
            paths.insert(utf8(theirs.location())?);
        }
    }
    Ok(paths
        .into_iter()
        .map(|path| MergeConflict { path })
        .collect())
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
