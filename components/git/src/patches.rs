use super::{
    BlameLine, BlameOptions, Entry, Error, FileDifference, FilePatch, MAX_BLOB_BYTES, MAX_ENTRIES,
    PatchOptions, commit_id, entries, repository_error, tree_id,
};
use gix::diff::blob::{
    Algorithm, Diff, InternedInput, UnifiedDiff,
    unified_diff::{ConsumeHunk, ContextSize, DiffLineKind, HunkHeader},
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 1_000_000;

pub(super) fn diff_renames(
    repo: &gix::Repository,
    before: &str,
    after: &str,
    max_files: u32,
) -> Result<Vec<FileDifference>, Error> {
    validate_file_limit(max_files)?;
    changes(repo, before, after, max_files, true)
}

fn validate_file_limit(limit: u32) -> Result<(), Error> {
    if !(1..=1000).contains(&limit) {
        return Err(Error::InvalidInput("max-files must be 1..=1000".into()));
    }
    Ok(())
}

fn changes(
    repo: &gix::Repository,
    before: &str,
    after: &str,
    limit: u32,
    renames: bool,
) -> Result<Vec<FileDifference>, Error> {
    let mut old = entries(repo, tree_id(repo, before)?)?;
    let mut new = entries(repo, tree_id(repo, after)?)?;
    for entry in old.values().chain(new.values()) {
        blame_path(&entry.path)?;
    }
    // Only deleted paths are rename sources: copies and edited renames aren't inferred.
    let mut sources: BTreeMap<(String, u32), BTreeSet<String>> = BTreeMap::new();
    if renames {
        for (path, entry) in &old {
            if !new.contains_key(path) {
                sources
                    .entry((entry.id.clone(), file_type(entry.mode)))
                    .or_default()
                    .insert(path.clone());
            }
        }
    }
    let mut out = Vec::new();
    if renames {
        let additions: Vec<_> = new
            .keys()
            .filter(|path| !old.contains_key(*path))
            .cloned()
            .collect();
        for path in additions {
            let entry = new.get(&path).ok_or_else(internal_error)?;
            let source = sources
                .get_mut(&(entry.id.clone(), file_type(entry.mode)))
                .and_then(BTreeSet::pop_first);
            if let Some(source) = source {
                out.push(FileDifference {
                    before: old.remove(&source),
                    after: new.remove(&path),
                    renamed: true,
                });
            }
        }
    }
    let paths: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    for path in paths {
        let before = old.remove(&path);
        let after = new.remove(&path);
        if before.as_ref().map(|e| (&e.id, e.mode)) != after.as_ref().map(|e| (&e.id, e.mode)) {
            out.push(FileDifference {
                before,
                after,
                renamed: false,
            });
        }
    }
    if out.len() > limit as usize {
        return Err(Error::Unsupported("changed files exceed max-files".into()));
    }
    out.sort_by(|a, b| difference_path(a).cmp(difference_path(b)));
    Ok(out)
}

fn difference_path(change: &FileDifference) -> &str {
    change
        .after
        .as_ref()
        .or(change.before.as_ref())
        .map_or("", |entry| &entry.path)
}

fn file_type(mode: u32) -> u32 {
    mode & 0o170_000
}

fn internal_error() -> Error {
    Error::Repository("inconsistent diff state".into())
}

fn blob(
    repo: &gix::Repository,
    entry: Option<&Entry>,
    budget: &mut usize,
) -> Result<Vec<u8>, Error> {
    let Some(entry) = entry else {
        return Ok(Vec::new());
    };
    if !matches!(entry.mode, 0o100_644 | 0o100_755 | 0o120_000) {
        return Err(Error::Unsupported(
            "submodules and special modes are unsupported".into(),
        ));
    }
    let id = gix::ObjectId::from_hex(entry.id.as_bytes()).map_err(repository_error)?;
    let blob = repo.find_blob(id).map_err(repository_error)?;
    *budget = budget.saturating_add(blob.data.len());
    if blob.data.len() > MAX_BLOB_BYTES || *budget > MAX_READ_BYTES {
        return Err(Error::Unsupported(
            "blob read limit exceeded (16 MiB/blob, 64 MiB/call)".into(),
        ));
    }
    Ok(blob.data.clone())
}

fn check_lines(bytes: &[u8]) -> Result<(), Error> {
    let lines = gix::diff::blob::sources::byte_lines(bytes).count();
    if lines > MAX_ENTRIES {
        return Err(Error::Unsupported("text exceeds 100000 lines".into()));
    }
    Ok(())
}

// Quote every ambiguous byte using Git's C-style path convention.
fn quote(path: &str) -> String {
    if path
        .bytes()
        .all(|b| (b'!'..=b'~').contains(&b) && b != b'"' && b != b'\\')
    {
        return path.to_owned();
    }
    let mut out = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b' '..=b'~' => out.push(char::from(byte)),
            _ => {
                out.push('\\');
                out.push(char::from(b'0' + (byte >> 6)));
                out.push(char::from(b'0' + ((byte >> 3) & 7)));
                out.push(char::from(b'0' + (byte & 7)));
            }
        }
    }
    out.push('"');
    out
}

#[derive(Debug)]
struct PatchWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for PatchWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("patch exceeds max-bytes"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ConsumeHunk for PatchWriter {
    type Out = Self;

    fn consume_hunk(
        &mut self,
        h: HunkHeader,
        lines: &[(DiffLineKind, &[u8])],
    ) -> std::io::Result<()> {
        // Git uses the preceding line for a zero-length range, not the next line.
        let old = if h.before_hunk_len == 0 {
            h.before_hunk_start - 1
        } else {
            h.before_hunk_start
        };
        let new = if h.after_hunk_len == 0 {
            h.after_hunk_start - 1
        } else {
            h.after_hunk_start
        };
        writeln!(
            self,
            "@@ -{old},{} +{new},{} @@",
            h.before_hunk_len, h.after_hunk_len
        )?;
        for (kind, line) in lines {
            self.write_all(&[match kind {
                DiffLineKind::Context => b' ',
                DiffLineKind::Add => b'+',
                DiffLineKind::Remove => b'-',
            }])?;
            self.write_all(line)?;
            if !line.ends_with(b"\n") {
                self.write_all(b"\n\\ No newline at end of file\n")?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Self {
        self
    }
}

fn patch_error(error: impl std::fmt::Display) -> Error {
    Error::Unsupported(error.to_string())
}

pub(super) fn unified_diff(
    repo: &gix::Repository,
    before: &str,
    after: &str,
    options: &PatchOptions,
) -> Result<Vec<FilePatch>, Error> {
    validate_file_limit(options.max_files)?;
    if options.context_lines > 100 || !(1..=16_777_216).contains(&options.max_bytes) {
        return Err(Error::InvalidInput(
            "context-lines must be 0..=100; max-bytes must be 1..=16777216".into(),
        ));
    }
    let changes = changes(
        repo,
        before,
        after,
        options.max_files,
        options.detect_renames,
    )?;
    let mut budget = 0;
    let mut remaining = options.max_bytes as usize;
    let mut out = Vec::new();
    for difference in changes {
        let old = blob(repo, difference.before.as_ref(), &mut budget)?;
        let new = blob(repo, difference.after.as_ref(), &mut budget)?;
        let binary = old.contains(&0) || new.contains(&0);
        let mut writer = PatchWriter {
            bytes: Vec::new(),
            limit: remaining,
        };
        let type_change = difference
            .before
            .as_ref()
            .zip(difference.after.as_ref())
            .is_some_and(|(a, b)| file_type(a.mode) != file_type(b.mode));
        if type_change {
            writer = render(
                writer,
                difference.before.as_ref(),
                None,
                &old,
                b"",
                false,
                options.context_lines,
            )?;
            writer = render(
                writer,
                None,
                difference.after.as_ref(),
                b"",
                &new,
                false,
                options.context_lines,
            )?;
        } else {
            writer = render(
                writer,
                difference.before.as_ref(),
                difference.after.as_ref(),
                &old,
                &new,
                difference.renamed,
                options.context_lines,
            )?;
        }
        remaining -= writer.bytes.len();
        out.push(FilePatch {
            difference,
            binary,
            patch: writer.bytes,
        });
    }
    Ok(out)
}

fn render(
    mut writer: PatchWriter,
    before: Option<&Entry>,
    after: Option<&Entry>,
    old: &[u8],
    new: &[u8],
    renamed: bool,
    context: u32,
) -> Result<PatchWriter, Error> {
    let a = before.or(after).ok_or_else(internal_error)?;
    let b = after.or(before).ok_or_else(internal_error)?;
    let old_path = quote(&format!("a/{}", a.path));
    let new_path = quote(&format!("b/{}", b.path));
    writeln!(writer, "diff --git {old_path} {new_path}").map_err(patch_error)?;
    match (before, after) {
        (None, Some(b)) => writeln!(writer, "new file mode {:06o}", b.mode),
        (Some(a), None) => writeln!(writer, "deleted file mode {:06o}", a.mode),
        (Some(a), Some(b)) if a.mode != b.mode => {
            writeln!(writer, "old mode {:06o}\nnew mode {:06o}", a.mode, b.mode)
        }
        _ => Ok(()),
    }
    .map_err(patch_error)?;
    if renamed {
        writeln!(
            writer,
            "similarity index 100%\nrename from {}\nrename to {}",
            quote(&a.path),
            quote(&b.path)
        )
        .map_err(patch_error)?;
    }
    let old_id = before.map_or("0000000000000000000000000000000000000000", |e| &e.id);
    let new_id = after.map_or("0000000000000000000000000000000000000000", |e| &e.id);
    write!(writer, "index {old_id}..{new_id}").map_err(patch_error)?;
    if before.zip(after).is_some_and(|(a, b)| a.mode == b.mode) {
        write!(writer, " {:06o}", a.mode).map_err(patch_error)?;
    }
    writeln!(writer).map_err(patch_error)?;
    if old == new {
        return Ok(writer);
    }
    let old_path = if before.is_some() {
        old_path
    } else {
        "/dev/null".into()
    };
    let new_path = if after.is_some() {
        new_path
    } else {
        "/dev/null".into()
    };
    if old.contains(&0) || new.contains(&0) {
        writeln!(writer, "Binary files {old_path} and {new_path} differ").map_err(patch_error)?;
        return Ok(writer);
    }
    check_lines(old)?;
    check_lines(new)?;
    writeln!(writer, "--- {old_path}\n+++ {new_path}").map_err(patch_error)?;
    let input = InternedInput::new(old, new);
    let diff = gix::diff::blob::diff_with_slider_heuristics(Algorithm::Histogram, &input);
    UnifiedDiff::new(&diff, &input, writer, ContextSize::symmetrical(context))
        .consume()
        .map_err(patch_error)
}

fn blame_path(path: &str) -> Result<(), Error> {
    // Reads accept non-portable but Git-valid UTF-8 paths, including ':' and '\'.
    if path.is_empty()
        || path.contains('\0')
        || path.split('/').count() > 128
        || path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.eq_ignore_ascii_case(".git")
        })
    {
        return Err(Error::InvalidInput(
            "invalid repository-relative file path".into(),
        ));
    }
    Ok(())
}

fn blame_entries(
    repo: &gix::Repository,
    id: gix::ObjectId,
    budget: &mut usize,
) -> Result<BTreeMap<String, Entry>, Error> {
    let tree = repo
        .find_commit(id)
        .map_err(repository_error)?
        .tree_id()
        .map_err(repository_error)?
        .detach();
    let entries = entries(repo, tree)?;
    *budget = budget.saturating_add(entries.len());
    if *budget > MAX_TREE_ENTRIES {
        return Err(Error::Unsupported(
            "blame tree-entry budget exceeded".into(),
        ));
    }
    Ok(entries)
}

fn text_blob(repo: &gix::Repository, entry: &Entry, budget: &mut usize) -> Result<Vec<u8>, Error> {
    if file_type(entry.mode) != 0o100_000 {
        return Err(Error::Unsupported("blame requires a regular file".into()));
    }
    let bytes = blob(repo, Some(entry), budget)?;
    if bytes.contains(&0) {
        return Err(Error::Unsupported("binary blame is unsupported".into()));
    }
    check_lines(&bytes)?;
    Ok(bytes)
}

fn line_number(index: usize) -> Result<u32, Error> {
    u32::try_from(index + 1).map_err(repository_error)
}

pub(super) fn blame(
    repo: &gix::Repository,
    revision: &str,
    file: &str,
    options: &BlameOptions,
) -> Result<Vec<BlameLine>, Error> {
    blame_path(file)?;
    if options.start_line == 0
        || !(1..=100_000).contains(&options.max_lines)
        || !(1..=1000).contains(&options.max_commits)
    {
        return Err(Error::InvalidInput(
            "blame requires start-line >= 1, max-lines 1..=100000, max-commits 1..=1000".into(),
        ));
    }
    let mut id = commit_id(repo, revision)?;
    let mut tree_budget = 0;
    let mut byte_budget = 0;
    let mut current_tree = blame_entries(repo, id, &mut tree_budget)?;
    let mut path = file.to_owned();
    let entry = current_tree
        .get(file)
        .ok_or_else(|| Error::InvalidInput("file not found".into()))?;
    let mut current = text_blob(repo, entry, &mut byte_budget)?;
    let count = gix::diff::blob::sources::byte_lines(&current).count();
    let start = options.start_line as usize - 1;
    if start >= count && !(count == 0 && start == 0) {
        return Err(Error::InvalidInput("start-line exceeds file length".into()));
    }
    let end = count.min(start.saturating_add(options.max_lines as usize));
    let mut pending: Vec<_> = (start..end).enumerate().collect();
    let mut output: Vec<Option<BlameLine>> = (start..end).map(|_| None).collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    for visited in 0..options.max_commits {
        let commit = repo.find_commit(id).map_err(repository_error)?;
        let decoded = commit.decode().map_err(repository_error)?;
        let parents: Vec<_> = decoded.parents().collect();
        if parents.len() > 1 {
            return Err(Error::Unsupported(
                "blame traversal through merges is unsupported".into(),
            ));
        }
        let parent = parents.first().copied();
        let mut parent_tree = BTreeMap::new();
        let mut parent_path = path.clone();
        if let Some(parent) = parent {
            if visited + 1 >= options.max_commits {
                return Err(Error::Unsupported(
                    "blame attribution exceeds max-commits".into(),
                ));
            }
            parent_tree = blame_entries(repo, parent, &mut tree_budget)?;
            if !parent_tree.contains_key(&path) {
                let entry = current_tree.get(&path).ok_or_else(internal_error)?;
                let candidates: Vec<_> = parent_tree
                    .values()
                    .filter(|old| {
                        old.id == entry.id
                            && file_type(old.mode) == file_type(entry.mode)
                            && !current_tree.contains_key(&old.path)
                    })
                    .collect();
                if candidates.len() > 1 {
                    return Err(Error::Unsupported(
                        "ambiguous clean rename origin in blame".into(),
                    ));
                }
                if let Some(old) = candidates.first() {
                    parent_path.clone_from(&old.path);
                }
            }
        }
        blame_path(&parent_path)?;
        let parent_entry = parent_tree.get(&parent_path);
        let previous = parent_entry
            .map(|e| text_blob(repo, e, &mut byte_budget))
            .transpose()?;
        let mapping = previous
            .as_ref()
            .map(|previous| unchanged_lines(previous, &current));
        let mut next = Vec::new();
        for (slot, line) in pending {
            if let Some(original) = mapping
                .as_ref()
                .and_then(|m| m.get(line))
                .copied()
                .flatten()
            {
                next.push((slot, original));
            } else {
                *output.get_mut(slot).ok_or_else(internal_error)? = Some(BlameLine {
                    line_number: line_number(start + slot)?,
                    commit: id.to_string(),
                    original_path: path.clone(),
                    original_line: line_number(line)?,
                });
            }
        }
        pending = next;
        if pending.is_empty() {
            return output
                .into_iter()
                .map(|line| line.ok_or_else(internal_error))
                .collect();
        }
        id = parent.ok_or_else(internal_error)?;
        current = previous.ok_or_else(internal_error)?;
        current_tree = parent_tree;
        path = parent_path;
    }
    Err(Error::Unsupported(
        "blame attribution exceeds max-commits".into(),
    ))
}

fn unchanged_lines(before: &[u8], after: &[u8]) -> Vec<Option<usize>> {
    let input = InternedInput::new(before, after);
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let mut mapping = vec![None; input.after.len()];
    let mut old = 0;
    let mut new = 0;
    for hunk in diff.hunks() {
        for (slot, line) in mapping
            .iter_mut()
            .take(hunk.after.start as usize)
            .skip(new)
            .zip(old..hunk.before.start as usize)
        {
            *slot = Some(line);
        }
        old = hunk.before.end as usize;
        new = hunk.after.end as usize;
    }
    for (slot, line) in mapping.iter_mut().skip(new).zip(old..input.before.len()) {
        *slot = Some(line);
    }
    mapping
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Guest;

    #[test]
    fn quoting_and_line_mapping_preserve_raw_bytes() {
        assert_eq!(quote("ordinary/name"), "ordinary/name");
        assert_eq!(quote("a\tb\nc\\d\"e"), "\"a\\tb\\nc\\\\d\\\"e\"");
        assert_eq!(quote("a\u{1}\u{7f}\u{e9}"), "\"a\\001\\177\\303\\251\"");
        assert_eq!(
            unchanged_lines(b"a\nb\nc\n", b"inserted\na\nB\nc\n"),
            vec![None, Some(0), None, Some(2)]
        );
        assert_eq!(unchanged_lines(b"a\n", b"a"), vec![None]);
        assert_eq!(unchanged_lines(b"", b""), Vec::<Option<usize>>::new());
    }

    #[test]
    fn text_and_output_limits_are_exact() {
        let mut bytes = b"x\n".repeat(100_000);
        check_lines(&bytes).unwrap();
        bytes.push(b'x');
        assert!(matches!(check_lines(&bytes), Err(Error::Unsupported(_))));
        let mut writer = PatchWriter {
            bytes: Vec::new(),
            limit: 2,
        };
        writer.write_all(b"ab").unwrap();
        assert!(writer.write_all(b"c").is_err());
        assert_eq!(writer.bytes, b"ab");
        assert!(validate_file_limit(1000).is_ok());
        assert!(validate_file_limit(1001).is_err());
    }

    #[test]
    fn blob_and_total_read_bounds_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repo.git");
        super::super::Component::init(path.to_str().unwrap().into(), "main".into()).unwrap();
        let repo = super::super::open(path.to_str().unwrap()).unwrap();
        let id = repo.write_blob(b"small").unwrap().to_string();
        let mut entry = Entry {
            path: "file".into(),
            id,
            mode: 0o100_644,
        };
        let mut budget = MAX_READ_BYTES - 5;
        assert_eq!(blob(&repo, Some(&entry), &mut budget).unwrap(), b"small");
        assert!(matches!(
            blob(&repo, Some(&entry), &mut budget),
            Err(Error::Unsupported(_))
        ));
        entry.id = repo
            .write_blob(vec![b'x'; MAX_BLOB_BYTES + 1])
            .unwrap()
            .to_string();
        assert!(matches!(
            blob(&repo, Some(&entry), &mut 0),
            Err(Error::Unsupported(_))
        ));
        entry.mode = 0o160_000;
        assert!(matches!(
            blob(&repo, Some(&entry), &mut 0),
            Err(Error::Unsupported(_))
        ));
    }
}
