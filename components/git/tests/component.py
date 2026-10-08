"""Exercise the real Wasm component against local Git repositories.

Git is used only as an interoperability oracle and fixture builder, never by
the component. Requires Python 3, Wasmtime, wasm-tools, and Git on PATH.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[3]
WASM = ROOT / "target/wasm32-wasip2/release/git.wasm"
WASMTIME = os.environ.get("WASMTIME", "wasmtime")
AUTHOR = '{name: "Agent", email: "agent@example.invalid", seconds: 1700000000, offset: 0}'


def run(*args):
    result = subprocess.run(args, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def string(value):
    return json.dumps(value, ensure_ascii=False)


def main():
    run("wasm-tools", "validate", "--features", "all", str(WASM))
    wit = run("wasm-tools", "component", "wit", str(WASM))
    imports = re.findall(r"^\s*import ([^;]+);", wit, re.MULTILINE)
    assert imports and all(name.startswith("wasi:") for name in imports), imports
    assert not any(":http/" in name or ":sockets/" in name for name in imports)
    assert "export yoshuawuyts:git/repository@0.1.0;" in wit

    # Keep fixtures under the worktree, including Windows CI runs.
    with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
        repo = Path(directory) / "agent.git"

        def invoke(expression, granted=True):
            args = [WASMTIME, "run"]
            if granted:
                args += ["--dir", f"{directory}::/repos"]
            args += ["--invoke", expression, str(WASM)]
            return run(*args)

        def ok(expression):
            output = invoke(expression)
            assert output == "ok" or output.startswith("ok("), output
            return output

        def error(expression, kind):
            output = invoke(expression)
            assert output.startswith(f"err({kind}("), output

        def native(*args):
            return run("git", f"--git-dir={repo}", *args)

        def native_at(repository, *args):
            return run("git", f"--git-dir={repository}", *args)

        def commit_on(repository_name, branch, parent, changes, message):
            expected = f"some({string(parent)})" if parent else "none"
            output = ok(
                f'commit-files("/repos/{repository_name}", {string(branch)}, {expected}, '
                f'[{changes}], {AUTHOR}, {string(message)})'
            )
            match = re.fullmatch(r'ok\("([0-9a-f]{40})"\)', output)
            assert match, output
            return match.group(1)

        def commit(parent, changes, message):
            return commit_on("agent.git", "main", parent, changes, message)

        ok('init("/repos/agent.git", "main")')
        error('init("/repos/agent.git", "main")', "repository")
        assert ok('references("/repos/agent.git")') == "ok([])"
        first = commit(
            None,
            '{path: "src/file", contents: some([0, 255, 10]), mode: regular}, '
            '{path: "script", contents: some([104, 105]), mode: executable}, '
            '{path: "link", contents: some([115, 114, 99]), mode: symlink}',
            "Initial commit",
        )
        assert native("rev-parse", "HEAD") == first
        assert ok('read-file("/repos/agent.git", "HEAD", "src/file")') == "ok([0, 255, 10])"
        assert f'id: "{first}"' in ok('log("/repos/agent.git", "HEAD", 10)')
        tree = ok('list-tree("/repos/agent.git", "HEAD")')
        assert all(f"mode: {mode}" in tree for mode in (33188, 33261, 40960)), tree
        ok('create-branch("/repos/agent.git", "feature/nested", "HEAD")')
        error('create-branch("/repos/agent.git", "feature/nested", "HEAD")', "conflict")
        assert "refs/heads/feature/nested" in ok('references("/repos/agent.git")')

        # Exercise the WASI byte-buffer mapping of packfiles, indexes and refs.
        native("repack", "-ad")
        native("pack-refs", "--all")
        native("commit-graph", "write", "--reachable")
        assert not (repo / "objects" / first[:2] / first[2:]).exists()
        assert first in ok('log("/repos/agent.git", "HEAD", 10)')
        assert ok('read-file("/repos/agent.git", "HEAD", "src/file")') == "ok([0, 255, 10])"

        second = commit(
            first,
            '{path: "src/file", contents: some([110, 101, 119]), mode: regular}, '
            '{path: "script", contents: none, mode: regular}',
            "Second commit",
        )
        assert native("rev-parse", "HEAD") == second
        assert native("rev-parse", "HEAD^") == first
        difference = ok(f'diff("/repos/agent.git", "{first}", "{second}")')
        assert 'path: "script"' in difference and 'path: "src/file"' in difference
        assert 'path: "link"' not in difference
        assert native("diff", "--name-only", first, second).splitlines() == ["script", "src/file"]
        stale = (
            f'commit-files("/repos/agent.git", "main", some("{first}"), '
            '[{path: "file", contents: some([1]), mode: regular}], '
            f'{AUTHOR}, "Stale commit")'
        )
        error(stale, "conflict")
        error('read-file("/repos/agent.git", "HEAD", "../config")', "invalid-input")
        error('log("/repos/agent.git", "HEAD", 0)', "invalid-input")
        assert invoke('resolve("/repos/agent.git", "HEAD")', granted=False).startswith("err(repository(")

        lock = repo / "refs/heads/main.lock"
        lock.write_text("held by another writer")
        error(
            f'commit-files("/repos/agent.git", "main", some("{second}"), '
            '[{path: "file", contents: some([1]), mode: regular}], '
            f'{AUTHOR}, "Locked commit")',
            "conflict",
        )
        assert lock.read_text() == "held by another writer"
        lock.unlink()
        assert native("rev-parse", "HEAD") == second
        native("fsck", "--strict", "--full")

        # A sparse object just over the mapping limit must fail before parsing.
        oversized_id = "1" * 40
        oversized = repo / "objects" / oversized_id[:2] / oversized_id[2:]
        oversized.parent.mkdir(exist_ok=True)
        with oversized.open("wb") as file:
            file.truncate(256 * 1024 * 1024 + 1)
        output = invoke(f'log("/repos/agent.git", "{oversized_id}", 1)')
        assert output.startswith("err(repository(") and "256 MiB" in output, output
        oversized.unlink()

        # Read ordinary repositories too, but do not bypass their index/worktree.
        worktree = Path(directory) / "worktree"
        run("git", "clone", "--quiet", str(repo), str(worktree))
        assert second in ok('resolve("/repos/worktree", "HEAD")')
        blob_id = run("git", "-C", str(worktree), "rev-parse", "HEAD:src/file")
        assert blob_id in ok('resolve("/repos/worktree", "HEAD:src/file")')
        error('resolve("/repos/worktree", ":src/file")', "unsupported")
        error('create-branch("/repos/worktree", "unsafe", "HEAD")', "unsupported")
        error(
            f'commit-files("/repos/worktree", "main", some("{second}"), '
            '[{path: "file", contents: some([1]), mode: regular}], '
            f'{AUTHOR}, "No worktree writes")',
            "unsupported",
        )

        def native_worktree(*args):
            return run("git", "-C", str(worktree), *args)

        assert native_worktree("status", "--porcelain") == ""
        (worktree / ".gitignore").write_text("*.ignored\n")
        (worktree / "staged.txt").write_text("staged\n")
        (worktree / "untracked.txt").write_text("untracked\n")
        (worktree / "hidden.ignored").write_text("ignored\n")
        ok('add("/repos/worktree", [".gitignore", "staged.txt"])')
        staged = ok('status("/repos/worktree")')
        assert 'path: ".gitignore"' in staged and "staged: some(added)" in staged, staged
        assert 'path: "hidden.ignored"' not in staged, staged
        assert native_worktree("diff", "--cached", "--name-only").splitlines() == [
            ".gitignore",
            "staged.txt",
        ]
        assert "A  .gitignore" in native_worktree("status", "--porcelain")
        ok('reset("/repos/worktree", ["staged.txt"])')
        assert native_worktree("diff", "--cached", "--name-only").splitlines() == [
            ".gitignore"
        ]
        status = ok('status("/repos/worktree")')
        assert 'path: "staged.txt"' in status and "untracked: true" in status, status
        ok('add("/repos/worktree", ["staged.txt"])')
        working_commit = ok(
            f'commit("/repos/worktree", some("{second}"), {AUTHOR}, "Working-tree commit")'
        )
        assert native_worktree("rev-parse", "HEAD") == working_commit[4:-2]
        assert native_worktree("diff", "--cached", "--name-only") == ""
        status = native_worktree("status", "--porcelain")
        assert status == "?? untracked.txt", repr(status)
        native_worktree("branch", "feature", second)
        (worktree / "hidden.ignored").unlink()

        ok('remove("/repos/worktree", ["staged.txt"])')
        assert not (worktree / "staged.txt").exists()
        assert native_worktree("diff", "--cached", "--name-status").splitlines() == [
            "D\tstaged.txt"
        ]
        (worktree / "src" / "file").write_bytes(b"local edit")
        error('checkout("/repos/worktree", "feature", false)', "conflict")
        ok('checkout("/repos/worktree", "feature", true)')
        assert native_worktree("rev-parse", "HEAD") == second
        status = native_worktree("status", "--porcelain")
        assert status == "?? untracked.txt", repr(status)
        native_worktree("fsck", "--strict", "--full")

        # Patches execute inside Wasm; native Git consumes the exact emitted bytes.
        patch_repo = Path(directory) / "patches"
        run("git", "init", "--quiet", "--initial-branch=main", str(patch_repo))

        def patch_git(*args):
            return run("git", "-C", str(patch_repo), *args)

        def patch_commit(message):
            patch_git("add", "-A")
            patch_git("-c", "user.name=Oracle", "-c", "user.email=oracle@example.invalid",
                      "commit", "--quiet", "-m", message)
            return patch_git("rev-parse", "HEAD")

        strange_paths = [
            "space name", "tab\tname", "line\nname", 'quo"te', "back\\slash",
            "colon:name", "caf\u00e9",
        ]
        for name in strange_paths:
            (patch_repo / name).write_bytes(b"old\n")
        (patch_repo / "edit").write_bytes(b"one\ntwo\nthree")
        (patch_repo / "old-name").write_bytes(b"clean rename\n")
        (patch_repo / "empty-delete").write_bytes(b"")
        (patch_repo / "mode").write_bytes(b"mode\n")
        (patch_repo / "type").write_bytes(b"target")
        (patch_repo / "raw").write_bytes(b"\xff\n")
        (patch_repo / "hunks").write_bytes(b"".join(f"line {i}\n".encode() for i in range(30)))
        (patch_repo / "insert").write_bytes(b"first\nlast\n")
        (patch_repo / "remove").write_bytes(b"first\nmiddle\nlast\n")
        (patch_repo / "crlf").write_bytes(b"first\r\nsecond\r\n")
        (patch_repo / "binary-old").write_bytes(b"\0unchanged")
        (patch_repo / "binary-mode").write_bytes(b"\0mode")
        (patch_repo / "link").symlink_to("old-target")
        p1 = patch_commit("Patch base")
        for name in strange_paths:
            (patch_repo / name).write_bytes(b"new\n")
        (patch_repo / "edit").write_bytes(b"one\nTWO\nthree\n")
        (patch_repo / "old-name").rename(patch_repo / "new-name")
        (patch_repo / "new-name").chmod(0o755)
        (patch_repo / "empty-delete").unlink()
        (patch_repo / "empty-add").write_bytes(b"")
        (patch_repo / "added").write_bytes(b"added without newline")
        (patch_repo / "mode").chmod(0o755)
        (patch_repo / "type").unlink()
        (patch_repo / "type").symlink_to("target")
        (patch_repo / "link").unlink()
        (patch_repo / "link").symlink_to("new-target")
        (patch_repo / "raw").write_bytes(b"\xfe\n")
        (patch_repo / "insert").write_bytes(b"first\nmiddle\nlast\n")
        (patch_repo / "remove").write_bytes(b"first\nlast\n")
        (patch_repo / "crlf").write_bytes(b"first\r\nSECOND\r\n")
        (patch_repo / "binary-old").rename(patch_repo / "binary-new")
        (patch_repo / "binary-mode").chmod(0o755)
        (patch_repo / "hunks").write_bytes(b"".join(
            (f"changed {i}\n" if i in (2, 25) else f"line {i}\n").encode()
            for i in range(30)
        ))
        p2 = patch_commit("Patch changes")

        def patch_expression(context, detect=True, max_files=1000, max_bytes=16777216):
            return (
                f'unified-diff("/repos/patches", "{p1}", "{p2}", '
                f'{{context-lines: {context}, max-files: {max_files}, '
                f'max-bytes: {max_bytes}, detect-renames: {str(detect).lower()}}})'
            )

        for context, detect in [(0, True), (3, True), (100, True)]:
            output = ok(patch_expression(context, detect))
            arrays = re.findall(r"patch: \[([\d,\s]*)\]", output)
            assert arrays, output
            patch = b"".join(bytes(int(n) for n in data.split(",") if n.strip()) for data in arrays)
            assert b"\\ No newline at end of file\n" in patch
            assert b'--- "a/line\\nname"' in patch
            assert b'--- "a/back\\\\slash"' in patch
            assert b'--- "a/caf\\303\\251"' in patch
            assert b"old mode 100644\nnew mode 100755" in patch
            if detect:
                assert b"rename from old-name\nrename to new-name" in patch
                assert b"rename from binary-old\nrename to binary-new" in patch
            patch_file = Path(directory) / "emitted.patch"
            patch_file.write_bytes(patch)
            patch_git("checkout", "--quiet", "--detach", p1)
            patch_git("apply", "--index", *(
                ["--unidiff-zero"] if context == 0 else []
            ), str(patch_file))
            assert patch_git("write-tree") == patch_git("rev-parse", f"{p2}^{{tree}}")
            patch_git("reset", "--quiet", "--hard", p1)
        no_renames = ok(patch_expression(3, False))
        assert "renamed: true" not in no_renames
        assert "binary: true" in no_renames
        error(patch_expression(101), "invalid-input")
        error(patch_expression(3, max_files=0), "invalid-input")
        error(patch_expression(3, max_files=1), "unsupported")
        error(patch_expression(3, max_bytes=1), "unsupported")
        error(patch_expression(3, max_bytes=0), "invalid-input")
        rename_output = ok(f'diff-renames("/repos/patches", "{p1}", "{p2}", 1000)')
        assert "renamed: true" in rename_output and 'path: "new-name"' in rename_output
        plain_output = ok(f'diff("/repos/patches", "{p1}", "{p2}")')
        assert 'path: "old-name"' in plain_output and 'path: "new-name"' in plain_output
        binary_output = ok(
            f'unified-diff("/repos/agent.git", "{first}", "{second}", '
            '{context-lines: 3, max-files: 1000, max-bytes: 16777216, detect-renames: true})'
        )
        assert "binary: true" in binary_output
        binary_arrays = re.findall(r"patch: \[([\d,\s]*)\]", binary_output)
        assert any(b"Binary files" in bytes(int(n) for n in data.split(",") if n.strip())
                   for data in binary_arrays)

        # Compare complete attribution (commit, original path/line) with Git porcelain.
        patch_git("checkout", "--quiet", "main")
        (patch_repo / "blame-old").write_bytes(b"a\nb\nc\n")
        b1 = patch_commit("Blame base")
        (patch_repo / "blame-old").write_bytes(b"inserted\na\nB\nc\n")
        b2 = patch_commit("Blame edit")
        (patch_repo / "blame-old").rename(patch_repo / "blame-new")
        b3 = patch_commit("Blame rename")
        (patch_repo / "blame-new").write_bytes(b"inserted\na\nB\nlast\n")
        b4 = patch_commit("Blame final")

        def blame_expression(file="blame-new", start=1, lines=100000, commits=1000, revision=b4):
            return (
                f'blame("/repos/patches", "{revision}", {string(file)}, '
                f'{{start-line: {start}, max-lines: {lines}, max-commits: {commits}}})'
            )

        output = ok(blame_expression())
        actual = [
            (int(final), commit, json.loads(original), int(line))
            for final, commit, original, line in re.findall(
                r'line-number: (\d+), commit: "([0-9a-f]{40})", '
                r'original-path: ("(?:[^"\\]|\\.)*"), original-line: (\d+)', output
            )
        ]
        porcelain = patch_git("blame", "--line-porcelain", b4, "--", "blame-new")
        expected = []
        header = None
        for line in porcelain.splitlines():
            match = re.match(r"^([0-9a-f]{40}) (\d+) (\d+)(?: \d+)?$", line)
            if match:
                header = match.groups()
            elif line.startswith("filename "):
                commit, original, final = header
                expected.append((int(final), commit, line[9:], int(original)))
        assert actual == expected, (actual, expected, output)
        assert actual == [
            (1, b2, "blame-old", 1), (2, b1, "blame-old", 1),
            (3, b2, "blame-old", 3), (4, b4, "blame-new", 4),
        ]
        assert "line-number: 2" in ok(blame_expression(start=2, lines=1))
        for file in strange_paths:
            assert p2 in ok(blame_expression(file=file))
        assert ok(blame_expression(file="empty-add")) == "ok([])"
        error(blame_expression(start=0), "invalid-input")
        error(blame_expression(start=5), "invalid-input")
        error(blame_expression(lines=0), "invalid-input")
        error(blame_expression(commits=0), "invalid-input")
        error(blame_expression(commits=2), "unsupported")
        error(blame_expression(file="../blame-new"), "invalid-input")
        error(blame_expression(file="missing"), "invalid-input")
        error(blame_expression(file="link"), "unsupported")
        error(
            f'blame("/repos/agent.git", "{first}", "src/file", '
            '{start-line: 1, max-lines: 100000, max-commits: 1000})',
            "unsupported",
        )
        ok('init("/repos/ambiguous.git", "main")')
        ambiguous_base = commit_on(
            "ambiguous.git", "main", None,
            '{path: "a", contents: some([120, 10]), mode: regular}, '
            '{path: "b", contents: some([120, 10]), mode: regular}',
            "Duplicate sources",
        )
        ambiguous_tip = commit_on(
            "ambiguous.git", "main", ambiguous_base,
            '{path: "a", contents: none, mode: regular}, '
            '{path: "b", contents: none, mode: regular}, '
            '{path: "c", contents: some([120, 10]), mode: regular}',
            "Ambiguous rename",
        )
        error(
            f'blame("/repos/ambiguous.git", "{ambiguous_tip}", "c", '
            '{start-line: 1, max-lines: 100000, max-commits: 1000})',
            "unsupported",
        )
        paired = ok(
            f'diff-renames("/repos/ambiguous.git", "{ambiguous_base}", "{ambiguous_tip}", 1000)'
        )
        assert re.search(r'before: some\(\{path: "a".*after: some\(\{path: "c".*renamed: true', paired)
        patch_git("repack", "-ad")
        assert b1 in ok(blame_expression())
        assert "renamed: true" in ok(f'diff-renames("/repos/patches", "{b2}", "{b3}", 1000)')
        (patch_repo / "too-many-lines").write_bytes(b"x\n" * 100001)
        oversized_text = patch_commit("Exceed text line bound")
        error(blame_expression(file="too-many-lines", revision=oversized_text), "unsupported")
        error(
            f'unified-diff("/repos/patches", "{b4}", "{oversized_text}", '
            '{context-lines: 3, max-files: 1000, max-bytes: 16777216, detect-renames: true})',
            "unsupported",
        )
        patch_git("update-index", "--add", "--cacheinfo", f"160000,{b4},submodule")
        patch_git("-c", "user.name=Oracle", "-c", "user.email=oracle@example.invalid",
                  "commit", "--quiet", "-m", "Submodule fixture")
        submodule_tip = patch_git("rev-parse", "HEAD")
        error(
            f'unified-diff("/repos/patches", "{oversized_text}", "{submodule_tip}", '
            '{context-lines: 3, max-files: 1000, max-bytes: 16777216, detect-renames: true})',
            "unsupported",
        )

        fast_forward_repo = Path(directory) / "fast-forward.git"
        ok('init("/repos/fast-forward.git", "main")')
        ff_base = commit_on(
            "fast-forward.git",
            "main",
            None,
            '{path: "base", contents: some([98, 97, 115, 101]), mode: regular}',
            "Base",
        )
        ok('create-branch("/repos/fast-forward.git", "topic", "HEAD")')
        ff_topic = commit_on(
            "fast-forward.git",
            "topic",
            ff_base,
            '{path: "topic", contents: some([116, 111, 112, 105, 99]), mode: regular}',
            "Topic",
        )
        ff = ok(
            f'merge-branch("/repos/fast-forward.git", "main", "topic", '
            f'"{ff_base}", "{ff_topic}", {AUTHOR}, "Fast-forward")'
        )
        assert ff == f"ok(fast-forward({string(ff_topic)}))", ff
        assert native_at(fast_forward_repo, "rev-parse", "refs/heads/main") == ff_topic
        native_at(fast_forward_repo, "fsck", "--strict", "--full")

        divergent_repo = Path(directory) / "divergent.git"
        ok('init("/repos/divergent.git", "main")')
        merge_base = commit_on(
            "divergent.git",
            "main",
            None,
            '{path: "base", contents: some([98, 97, 115, 101]), mode: regular}',
            "Base",
        )
        ok('create-branch("/repos/divergent.git", "topic", "HEAD")')
        main_tip = commit_on(
            "divergent.git",
            "main",
            merge_base,
            '{path: "main", contents: some([109, 97, 105, 110]), mode: regular}',
            "Main",
        )
        topic_tip = commit_on(
            "divergent.git",
            "topic",
            merge_base,
            '{path: "topic", contents: some([116, 111, 112, 105, 99]), mode: regular}',
            "Topic",
        )
        merged = ok(
            f'merge-branch("/repos/divergent.git", "main", "topic", '
            f'"{main_tip}", "{topic_tip}", {AUTHOR}, "Merge topic")'
        )
        merge_match = re.fullmatch(r'ok\(merged\("([0-9a-f]{40})"\)\)', merged)
        assert merge_match, merged
        merged_tip = merge_match.group(1)
        error(
            f'blame("/repos/divergent.git", "{merged_tip}", "base", '
            '{start-line: 1, max-lines: 100000, max-commits: 1000})',
            "unsupported",
        )
        assert native_at(divergent_repo, "rev-parse", "refs/heads/main") == merged_tip
        assert native_at(divergent_repo, "show", "-s", "--format=%P", merged_tip).split() == [
            main_tip,
            topic_tip,
        ]
        error(
            f'merge-branch("/repos/divergent.git", "main", "topic", "{merge_base}", '
            f'"{topic_tip}", {AUTHOR}, "Stale merge")',
            "conflict",
        )
        assert native_at(divergent_repo, "rev-parse", "refs/heads/main") == merged_tip
        native_at(divergent_repo, "fsck", "--strict", "--full")

        conflict_repo = Path(directory) / "merge-conflict.git"
        ok('init("/repos/merge-conflict.git", "main")')
        conflict_base = commit_on(
            "merge-conflict.git",
            "main",
            None,
            '{path: "file", contents: some([98, 97, 115, 101]), mode: regular}',
            "Base",
        )
        ok('create-branch("/repos/merge-conflict.git", "topic", "HEAD")')
        conflict_main = commit_on(
            "merge-conflict.git",
            "main",
            conflict_base,
            '{path: "file", contents: some([109, 97, 105, 110]), mode: regular}',
            "Main",
        )
        conflict_topic = commit_on(
            "merge-conflict.git",
            "topic",
            conflict_base,
            '{path: "file", contents: some([116, 111, 112, 105, 99]), mode: regular}',
            "Topic",
        )
        conflict = ok(
            f'merge-branch("/repos/merge-conflict.git", "main", "topic", '
            f'"{conflict_main}", "{conflict_topic}", {AUTHOR}, "Conflict")'
        )
        assert "conflicts" in conflict and 'path: "file"' in conflict, conflict
        assert native_at(conflict_repo, "rev-parse", "refs/heads/main") == conflict_main
        native_at(conflict_repo, "fsck", "--strict", "--full")

        rebase_repo = Path(directory) / "rebase.git"
        ok('init("/repos/rebase.git", "main")')
        rebase_base = commit_on(
            "rebase.git",
            "main",
            None,
            '{path: "base", contents: some([98, 97, 115, 101]), mode: regular}',
            "Base",
        )
        ok('create-branch("/repos/rebase.git", "topic", "HEAD")')
        original_topic = commit_on(
            "rebase.git",
            "topic",
            rebase_base,
            '{path: "topic", contents: some([116, 111, 112, 105, 99]), mode: regular}',
            "Topic",
        )
        rebase_onto = commit_on(
            "rebase.git",
            "main",
            rebase_base,
            '{path: "main", contents: some([109, 97, 105, 110]), mode: regular}',
            "Main",
        )
        committer = '{name: "Rebaser", email: "rebaser@example.invalid", seconds: 1800000000, offset: 0}'
        rebased = ok(
            f'rebase-branch("/repos/rebase.git", "topic", "{original_topic}", '
            f'"{rebase_onto}", {committer})'
        )
        rebase_match = re.search(r'tip: "([0-9a-f]{40})"', rebased)
        assert rebase_match and "rebased" in rebased, rebased
        rebased_tip = rebase_match.group(1)
        assert native_at(rebase_repo, "rev-parse", "refs/heads/topic") == rebased_tip
        assert native_at(rebase_repo, "show", "-s", "--format=%P", rebased_tip) == rebase_onto
        assert native_at(rebase_repo, "show", "-s", "--format=%cn%x00%ce", rebased_tip) == (
            "Rebaser\x00rebaser@example.invalid"
        )
        error(
            f'rebase-branch("/repos/rebase.git", "topic", "{original_topic}", '
            f'"{original_topic}", {committer})',
            "conflict",
        )
        assert native_at(rebase_repo, "rev-parse", "refs/heads/topic") == rebased_tip
        native_at(rebase_repo, "fsck", "--strict", "--full")

        rebase_conflict_repo = Path(directory) / "rebase-conflict.git"
        ok('init("/repos/rebase-conflict.git", "main")')
        rebase_conflict_base = commit_on(
            "rebase-conflict.git",
            "main",
            None,
            '{path: "file", contents: some([98, 97, 115, 101]), mode: regular}',
            "Base",
        )
        ok('create-branch("/repos/rebase-conflict.git", "topic", "HEAD")')
        rebase_conflict_topic = commit_on(
            "rebase-conflict.git",
            "topic",
            rebase_conflict_base,
            '{path: "file", contents: some([116, 111, 112, 105, 99]), mode: regular}',
            "Topic",
        )
        rebase_conflict_onto = commit_on(
            "rebase-conflict.git",
            "main",
            rebase_conflict_base,
            '{path: "file", contents: some([109, 97, 105, 110]), mode: regular}',
            "Main",
        )
        rebase_conflict = ok(
            f'rebase-branch("/repos/rebase-conflict.git", "topic", '
            f'"{rebase_conflict_topic}", "{rebase_conflict_onto}", {committer})'
        )
        assert "conflicts" in rebase_conflict, rebase_conflict
        assert rebase_conflict_topic in rebase_conflict and 'paths: ["file"]' in rebase_conflict, rebase_conflict
        assert (
            native_at(rebase_conflict_repo, "rev-parse", "refs/heads/topic")
            == rebase_conflict_topic
        )
        native_at(rebase_conflict_repo, "fsck", "--strict", "--full")

    print("Git component: Wasm patches/blame, merge/rebase, CAS writes, and Git interoperability passed.")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.stderr.write(error.stderr)
        raise
