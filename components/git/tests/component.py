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
    return json.dumps(value)


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

        def commit(parent, changes, message):
            expected = f"some({string(parent)})" if parent else "none"
            output = ok(
                f'commit-files("/repos/agent.git", "main", {expected}, '
                f'[{changes}], {AUTHOR}, {string(message)})'
            )
            match = re.fullmatch(r'ok\("([0-9a-f]{40})"\)', output)
            assert match, output
            return match.group(1)

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

    print("Git component: Wasm execution, packed objects, CAS writes, and Git interoperability passed.")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.stderr.write(error.stderr)
        raise
