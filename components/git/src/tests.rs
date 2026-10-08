use super::*;

fn author() -> Signature {
    Signature {
        name: "Agent".into(),
        email: "agent@example.invalid".into(),
        seconds: 1_700_000_000,
        offset: 3600,
    }
}

fn change(path: &str, contents: Option<&[u8]>, mode: FileMode) -> Change {
    Change {
        path: path.into(),
        contents: contents.map(<[u8]>::to_vec),
        mode,
    }
}

fn repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo.git").to_str().unwrap().to_owned();
    Component::init(path.clone(), "main".into()).unwrap();
    (dir, path)
}

fn commit(path: &str, parent: Option<String>, changes: Vec<Change>) -> String {
    Component::commit_files(
        path.into(),
        "main".into(),
        parent,
        changes,
        author(),
        "message\n".into(),
    )
    .unwrap()
}

#[test]
fn roundtrip_binary_nested_files_history_and_modes() {
    let (_dir, path) = repo();
    let first = commit(
        &path,
        None,
        vec![
            change("src/file", Some(b"\0\xff\n"), FileMode::Regular),
            change("script", Some(b"echo hi\n"), FileMode::Executable),
            change("link", Some(b"src/file"), FileMode::Symlink),
        ],
    );
    assert_eq!(
        Component::resolve(path.clone(), "HEAD".into()).unwrap(),
        first
    );
    assert_eq!(
        Component::read_file(path.clone(), first.clone(), "src/file".into()).unwrap(),
        b"\0\xff\n"
    );
    let entries = Component::list_tree(path.clone(), "HEAD".into()).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.path.as_str(), e.mode))
            .collect::<Vec<_>>(),
        vec![
            ("link", 0o120_000),
            ("script", 0o100_755),
            ("src/file", 0o100_644)
        ]
    );
    let second = commit(
        &path,
        Some(first.clone()),
        vec![
            change("src/file", Some(b"new"), FileMode::Regular),
            change("script", None, FileMode::Regular),
        ],
    );
    let log = Component::log(path.clone(), "HEAD".into(), 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log.first().unwrap().id, second);
    assert_eq!(log.first().unwrap().parents, vec![first.clone()]);
    assert_eq!(log.first().unwrap().author.offset, 3600);
    assert_eq!(log.first().unwrap().message, b"message\n");
    let differences = Component::diff(path.clone(), first, second).unwrap();
    assert_eq!(differences.len(), 2);
    assert_eq!(differences.first().unwrap().path, "script");
    assert!(differences.first().unwrap().after.is_none());
    assert!(Component::read_file(path, "HEAD".into(), "script".into()).is_err());
}

#[test]
fn branches_conflicts_and_noop_commits() {
    let (_dir, path) = repo();
    let first = commit(
        &path,
        None,
        vec![change("file", Some(b"a"), FileMode::Regular)],
    );
    Component::create_branch(path.clone(), "feature/nested".into(), "HEAD".into()).unwrap();
    assert!(
        Component::create_branch(path.clone(), "feature/nested".into(), "HEAD".into()).is_err()
    );
    let refs = Component::references(path.clone()).unwrap();
    assert_eq!(
        refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["refs/heads/feature/nested", "refs/heads/main"]
    );
    assert!(matches!(
        Component::commit_files(
            path.clone(),
            "main".into(),
            None,
            vec![change("file", Some(b"b"), FileMode::Regular)],
            author(),
            "stale".into()
        ),
        Err(Error::Conflict(_))
    ));
    assert!(
        Component::commit_files(
            path.clone(),
            "main".into(),
            Some(first.clone()),
            vec![change("file", Some(b"a"), FileMode::Regular)],
            author(),
            "noop".into()
        )
        .is_err()
    );
    let second = commit(
        &path,
        Some(first.clone()),
        vec![change("file", Some(b"b"), FileMode::Regular)],
    );
    assert!(matches!(
        Component::commit_files(
            path.clone(),
            "main".into(),
            Some(first),
            vec![change("file", Some(b"c"), FileMode::Regular)],
            author(),
            "stale".into()
        ),
        Err(Error::Conflict(_))
    ));
    assert_eq!(Component::resolve(path, "main".into()).unwrap(), second);
}

#[test]
fn invalid_inputs_are_rejected_before_writing() {
    let (_dir, path) = repo();
    for branch in ["", "HEAD", "../bad", "a.lock", "a..b", "-bad", "@", "a\nb"] {
        assert!(branch_name(branch).is_err(), "{branch}");
    }
    for file in [
        "",
        "/file",
        "../file",
        "a/../b",
        ".git/config",
        "a/.GIT/b",
        "a//b",
        "a\\b",
        "C:file",
    ] {
        assert!(validate_file_path(file).is_err(), "{file}");
    }
    assert!(
        validate_changes(&[
            change("a", Some(b""), FileMode::Regular),
            change("a/b", Some(b""), FileMode::Regular),
        ])
        .is_err()
    );
    assert!(
        validate_changes(&[
            change("a", Some(b""), FileMode::Regular),
            change("a", Some(b""), FileMode::Regular),
        ])
        .is_err()
    );
    assert!(Component::init(path.clone(), "main".into()).is_err());
    assert!(Component::log(path.clone(), "HEAD".into(), 0).is_err());
    assert!(Component::log(path.clone(), "HEAD".into(), 1001).is_err());
    assert!(
        Component::commit_files(
            path.clone(),
            "main".into(),
            None,
            vec![],
            author(),
            "empty".into()
        )
        .is_err()
    );
    assert!(Component::references(path).unwrap().is_empty());
}

#[test]
fn file_mode_changes_are_differences_and_last_file_can_be_deleted() {
    let (_dir, path) = repo();
    let first = commit(
        &path,
        None,
        vec![change("dir/file", Some(b"a"), FileMode::Regular)],
    );
    let second = commit(
        &path,
        Some(first.clone()),
        vec![change("dir/file", Some(b"a"), FileMode::Executable)],
    );
    let diff = Component::diff(path.clone(), first, second.clone()).unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(
        diff.first().unwrap().before.as_ref().unwrap().mode,
        0o100_644
    );
    assert_eq!(
        diff.first().unwrap().after.as_ref().unwrap().mode,
        0o100_755
    );
    commit(
        &path,
        Some(second),
        vec![change("dir/file", None, FileMode::Regular)],
    );
    assert!(
        Component::list_tree(path, "HEAD".into())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn missing_files_directories_and_invalid_signatures_are_rejected() {
    let (_dir, path) = repo();
    let first = commit(
        &path,
        None,
        vec![change("dir/file", Some(b"a"), FileMode::Regular)],
    );
    for changes in [
        vec![change("missing", None, FileMode::Regular)],
        vec![change("dir", None, FileMode::Regular)],
        vec![change("dir", Some(b"b"), FileMode::Regular)],
        vec![change("dir/file/sub", Some(b"b"), FileMode::Regular)],
    ] {
        assert!(
            Component::commit_files(
                path.clone(),
                "main".into(),
                Some(first.clone()),
                changes,
                author(),
                "bad".into()
            )
            .is_err()
        );
    }
    let mut bad_author = author();
    bad_author.name = "Injected\ncommitter".into();
    assert!(encode_signature(bad_author).is_err());
    let mut bad_author = author();
    bad_author.offset = 1;
    assert!(encode_signature(bad_author).is_err());
    assert_eq!(Component::resolve(path, "HEAD".into()).unwrap(), first);
}

#[test]
fn a_failed_compare_and_swap_preserves_reference_and_cleans_its_lock() {
    let (_dir, path) = repo();
    let first = commit(
        &path,
        None,
        vec![change("file", Some(b"a"), FileMode::Regular)],
    );
    let repo = open(&path).unwrap();
    let id = gix::ObjectId::from_hex(first.as_bytes()).unwrap();
    assert!(matches!(
        update_branch(&repo, "refs/heads/main", id, None),
        Err(Error::Conflict(_))
    ));
    assert!(
        !std::path::Path::new(&path)
            .join("refs/heads/main.lock")
            .exists()
    );
    assert_eq!(Component::resolve(path, "HEAD".into()).unwrap(), first);
}

#[test]
fn limits_are_enforced_without_creating_a_reference() {
    let (_dir, path) = repo();
    let changes = vec![Change {
        path: "too-large".into(),
        contents: Some(vec![0; MAX_BLOB_BYTES + 1]),
        mode: FileMode::Regular,
    }];
    assert!(
        Component::commit_files(
            path.clone(),
            "main".into(),
            None,
            changes,
            author(),
            "large".into()
        )
        .is_err()
    );
    assert!(validate_file_path(&vec!["a"; MAX_DEPTH + 1].join("/")).is_err());
    let changes: Vec<_> = (0..1001)
        .map(|i| change(&format!("file{i}"), Some(b""), FileMode::Regular))
        .collect();
    assert!(validate_changes(&changes).is_err());
    assert!(Component::references(path).unwrap().is_empty());
}

#[test]
fn working_tree_staging_status_commit_and_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("worktree");
    gix::ThreadSafeRepository::init(
        &path,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
    )
    .unwrap();
    let path = path.to_str().unwrap().to_owned();
    std::fs::write(
        std::path::Path::new(&path).join(".gitignore"),
        "*.ignored\n",
    )
    .unwrap();
    std::fs::write(std::path::Path::new(&path).join("file"), b"first\n").unwrap();
    std::fs::write(
        std::path::Path::new(&path).join("hidden.ignored"),
        b"ignored\n",
    )
    .unwrap();
    Component::add(path.clone(), vec![".gitignore".into(), "file".into()]).unwrap();
    let first = Component::commit(path.clone(), None, author(), "initial\n".into()).unwrap();
    assert!(Component::status(path.clone()).unwrap().is_empty());

    std::fs::write(std::path::Path::new(&path).join("file"), b"second\n").unwrap();
    std::fs::write(std::path::Path::new(&path).join("new"), b"untracked\n").unwrap();
    let status = Component::status(path.clone()).unwrap();
    assert!(
        status
            .iter()
            .any(|item| { item.path == "file" && item.unstaged == Some(ChangeKind::Modified) })
    );
    assert!(
        status
            .iter()
            .any(|item| item.path == "new" && item.untracked)
    );
    assert!(!status.iter().any(|item| item.path == "hidden.ignored"));
    assert!(matches!(
        Component::checkout(path.clone(), first.clone(), false),
        Err(Error::Conflict(_))
    ));

    Component::add(path.clone(), vec!["file".into()]).unwrap();
    Component::reset(path.clone(), vec!["file".into()]).unwrap();
    let status = Component::status(path.clone()).unwrap();
    assert!(status.iter().any(|item| item.path == "file"
        && item.staged.is_none()
        && item.unstaged == Some(ChangeKind::Modified)));

    Component::add(path.clone(), vec!["file".into()]).unwrap();
    let second = Component::commit(
        path.clone(),
        Some(first.clone()),
        author(),
        "second\n".into(),
    )
    .unwrap();
    assert_eq!(
        Component::resolve(path.clone(), "HEAD".into()).unwrap(),
        second
    );
    let file = std::path::Path::new(&path).join("file");
    std::fs::write(&file, b"uncommitted edit\n").unwrap();
    assert!(matches!(
        Component::remove(path.clone(), vec!["file".into()]),
        Err(Error::Conflict(_))
    ));
    assert_eq!(std::fs::read(&file).unwrap(), b"uncommitted edit\n");
    std::fs::write(&file, b"second\n").unwrap();
    Component::remove(path.clone(), vec!["file".into()]).unwrap();
    assert!(!file.exists());
    assert!(
        Component::status(path.clone())
            .unwrap()
            .iter()
            .any(|item| item.path == "file" && item.staged == Some(ChangeKind::Removed))
    );

    Component::checkout(path.clone(), first, true).unwrap();
    assert_eq!(
        std::fs::read(std::path::Path::new(&path).join("file")).unwrap(),
        b"first\n"
    );
    assert!(std::path::Path::new(&path).join("new").exists());
    assert!(
        Component::status(path)
            .unwrap()
            .iter()
            .any(|item| item.path == "new" && item.untracked)
    );
}
