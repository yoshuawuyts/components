"""Exercise the real Wasm component against local Git repositories.

Git is used only as an interoperability oracle and fixture builder, never by
the component. Requires Python 3, Wasmtime, wasm-tools, and Git on PATH.
"""

import json
import os
from pathlib import Path
import re
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import subprocess
import sys
import tempfile
import threading
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parents[3]
WASM = ROOT / "target/wasm32-wasip2/release/git.wasm"
WASMTIME = os.environ.get("WASMTIME", "wasmtime")
AUTHOR = '{name: "Agent", email: "agent@example.invalid", seconds: 1700000000, offset: 0}'


class GitHTTPServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, root, auth=None):
        self.root = Path(root)
        self.auth = auth
        self.seen_authorization = []
        self.fault = None
        self.redirect_url = None
        self.pause_post = False
        self.post_received = threading.Event()
        self.release_post = threading.Event()
        super().__init__(("127.0.0.1", 0), GitHTTPHandler)


class GitHTTPHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.dispatch()

    def do_POST(self):
        self.dispatch()

    def reply(self, status, content_type, body, headers=()):
        self.send_response(status)
        if content_type:
            self.send_header("Content-Type", content_type)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def dispatch(self):
        fixture = self.server
        fixture.seen_authorization.append(self.headers.get("Authorization"))
        if fixture.auth:
            expected = "Basic " + base64.b64encode(
                f"{fixture.auth[0]}:{fixture.auth[1]}".encode()
            ).decode()
            expected_bearer = "Bearer " + fixture.auth[1]
            if self.headers.get("Authorization") not in (expected, expected_bearer):
                self.reply(
                    401,
                    "text/plain",
                    b"authentication required\n",
                    [("WWW-Authenticate", 'Basic realm="fixture"')],
                )
                return
        if fixture.fault == "http-error":
            self.reply(500, "text/plain", b"fixture failure\n")
            return
        if fixture.fault == "redirect":
            self.reply(
                302,
                "text/plain",
                b"redirects are not followed\n",
                [("Location", fixture.redirect_url)],
            )
            return
        path = urlsplit(self.path).path
        if fixture.fault == "oversized" and path.endswith("/info/refs"):
            self.reply(
                200,
                "application/x-git-upload-pack-advertisement",
                b"x" * 1024,
            )
            return
        if fixture.fault == "malformed" and path.endswith("/info/refs"):
            self.reply(200, "application/x-git-upload-pack-advertisement", b"0003")
            return

        request_length = int(self.headers.get("Content-Length", "0"))
        request_body = self.rfile.read(request_length) if request_length else b""
        env = os.environ.copy()
        env.update(
            {
                "GIT_PROJECT_ROOT": str(fixture.root),
                "GIT_HTTP_EXPORT_ALL": "1",
                "PATH_INFO": path,
                "QUERY_STRING": urlsplit(self.path).query,
                "REQUEST_METHOD": self.command,
                "CONTENT_TYPE": self.headers.get("Content-Type", ""),
                "CONTENT_LENGTH": str(request_length),
                "REMOTE_ADDR": self.client_address[0],
                "REMOTE_USER": fixture.auth[0] if fixture.auth else "",
                "AUTH_TYPE": "Basic" if fixture.auth else "",
                "SERVER_PROTOCOL": self.protocol_version,
                "SERVER_NAME": "127.0.0.1",
                "SERVER_PORT": str(fixture.server_address[1]),
            }
        )
        result = subprocess.run(
            ["git", "http-backend"],
            input=request_body,
            capture_output=True,
            env=env,
            timeout=30,
        )
        if result.returncode:
            self.reply(500, "text/plain", b"git http-backend failed\n")
            return
        output = result.stdout
        separator = output.find(b"\r\n\r\n")
        separator_length = 4
        if separator < 0:
            separator = output.find(b"\n\n")
            separator_length = 2
        if separator < 0:
            self.reply(500, "text/plain", b"invalid CGI response\n")
            return
        raw_headers = output[:separator].splitlines()
        body = output[separator + separator_length :]
        status = 200
        headers = []
        for line in raw_headers:
            name, _, value = line.partition(b":")
            if not _:
                continue
            if name.lower() == b"status":
                status = int(value.strip().split(maxsplit=1)[0])
            elif name.lower() not in (b"content-length", b"connection", b"transfer-encoding"):
                headers.append((name.decode("ascii"), value.decode("latin1").strip()))
        content_type = next(
            (value for name, value in headers if name.lower() == "content-type"),
            "application/octet-stream",
        )
        if fixture.fault == "bad-pack" and self.command == "POST":
            body = bytearray(body)
            offset = 0
            last_pack_byte = None
            while offset + 4 <= len(body):
                length = int(bytes(body[offset : offset + 4]), 16)
                if length == 0:
                    offset += 4
                    continue
                end = offset + length
                if end > len(body):
                    break
                if length > 5 and body[offset + 4] == 1:
                    last_pack_byte = end - 1
                offset = end
            if last_pack_byte is not None:
                body[last_pack_byte] ^= 1
                body = bytes(body)
            else:
                self.reply(500, "text/plain", b"fixture could not corrupt pack\n")
                return
        if self.command == "POST" and fixture.pause_post:
            fixture.post_received.set()
            if not fixture.release_post.wait(30):
                self.reply(500, "text/plain", b"fixture pause timed out\n")
                return
            fixture.pause_post = False
        self.reply(status, content_type, body, headers)


def run(*args):
    result = subprocess.run(args, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def string(value):
    return json.dumps(value, ensure_ascii=False)


def network_options(response=16777216, pack=16777216, hosts=None, credentials="none"):
    hosts = hosts or ["127.0.0.1"]
    rendered_hosts = "[" + ", ".join(string(host) for host in hosts) + "]"
    return (
        f"{{max-response-bytes: {response}, max-pack-bytes: {pack}, max-refs: 1000, "
        f"allowed-hosts: {rendered_hosts}, credentials: {credentials}}}"
    )


def exercise_network_component(directory, invoke, ok, error, native_at):
    projects = Path(directory) / "http-projects"
    projects.mkdir()
    remote = projects / "source.git"
    worktree = Path(directory) / "http-worktree"
    run("git", "init", "--bare", "--initial-branch=main", str(remote))
    run("git", "init", "--initial-branch=main", str(worktree))
    run("git", "-C", str(worktree), "config", "user.name", "Oracle")
    run("git", "-C", str(worktree), "config", "user.email", "oracle@example.invalid")
    (worktree / "readme.txt").write_text("base\n")
    run("git", "-C", str(worktree), "add", "readme.txt")
    run("git", "-C", str(worktree), "commit", "--quiet", "-m", "Base")
    run("git", "-C", str(worktree), "remote", "add", "origin", str(remote))
    run("git", "-C", str(worktree), "push", "--quiet", "-u", "origin", "main")
    first = run("git", "-C", str(worktree), "rev-parse", "HEAD")

    run("git", "-C", str(worktree), "switch", "--quiet", "-c", "feature/nested")
    (worktree / "feature.txt").write_text("feature\n")
    run("git", "-C", str(worktree), "add", "feature.txt")
    run("git", "-C", str(worktree), "commit", "--quiet", "-m", "Feature")
    run("git", "-C", str(worktree), "push", "--quiet", "-u", "origin", "feature/nested")
    run("git", "-C", str(worktree), "tag", "-a", "v1.0", "-m", "Release one")
    run("git", "-C", str(worktree), "push", "--quiet", "origin", "v1.0")
    feature = run("git", "-C", str(worktree), "rev-parse", "refs/heads/feature/nested")
    run("git", "--git-dir", str(remote), "gc", "--prune=now")
    run("git", "--git-dir", str(remote), "pack-refs", "--all")
    run("git", "--git-dir", str(remote), "fsck", "--strict", "--full")

    empty_remote = projects / "empty.git"
    run("git", "init", "--bare", "--initial-branch=main", str(empty_remote))
    server = GitHTTPServer(projects)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_address[1]}"

    def clone_expr(destination, project="source.git", opts=None):
        return (
            f"clone({string('/repos/' + destination)}, "
            f"{string(base_url + '/' + project)}, \"origin\", "
            f"{opts or network_options()})"
        )

    def fetch_expr(destination, opts=None):
        return f'fetch({string("/repos/" + destination)}, "origin", {opts or network_options()})'

    def git_dir(repository, *args):
        return native_at(Path(directory) / repository, *args)

    try:
        # HTTP imports require the explicit Wasmtime host capability.
        denied = subprocess.run(
            [
                WASMTIME,
                "run",
                "--dir",
                f"{directory}::/repos",
                "--invoke",
                clone_expr("http-denied.git"),
                str(WASM),
            ],
            capture_output=True,
            text=True,
        )
        assert denied.returncode != 0, denied.stdout
        assert "http" in (denied.stderr + denied.stdout).lower(), denied.stderr

        # The caller's hostname allowlist is enforced before any HTTP request.
        denied_host = invoke(
            clone_expr("unlisted.git", opts=network_options(hosts=["example.invalid"]))
        )
        assert denied_host.startswith("err(invalid-input("), denied_host
        assert not (Path(directory) / "unlisted.git").exists()

        cloned = ok(clone_expr("network.git"))
        assert "default-branch: \"main\"" in cloned, cloned
        cloned_repo = Path(directory) / "network.git"
        assert git_dir("network.git", "rev-parse", "refs/heads/main") == first
        assert git_dir("network.git", "rev-parse", "refs/remotes/origin/main") == first
        assert git_dir("network.git", "rev-parse", "refs/remotes/origin/feature/nested") == feature
        assert git_dir("network.git", "rev-parse", "refs/tags/v1.0") == git_dir(
            remote, "rev-parse", "refs/tags/v1.0"
        )
        assert git_dir("network.git", "ls-tree", "-r", "--name-only", "refs/remotes/origin/main") == (
            "readme.txt"
        )
        git_dir("network.git", "fsck", "--strict", "--full")
        git_dir("network.git", "update-ref", "refs/tags/v1.0", feature)

        empty = ok(clone_expr("empty.git", project="empty.git"))
        assert "references: []" in empty, empty
        assert git_dir("empty.git", "symbolic-ref", "HEAD") == "refs/heads/main"
        assert git_dir("empty.git", "for-each-ref") == ""

        # Authentication is supplied only to this call and never stored in config or output.
        auth_server = GitHTTPServer(projects, ("agent", "secret"))
        auth_thread = threading.Thread(target=auth_server.serve_forever, daemon=True)
        auth_thread.start()
        auth_url = f"http://127.0.0.1:{auth_server.server_address[1]}/source.git"
        unauthenticated = invoke(
            f'clone("/repos/unauthenticated.git", {string(auth_url)}, "origin", '
            f"{network_options()})"
        )
        assert unauthenticated.startswith("err(network(authentication-required))"), unauthenticated
        basic = 'some({username: some("agent"), password: some("secret"), bearer-token: none})'
        basic_result = ok(
            f'clone("/repos/auth-basic.git", {string(auth_url)}, "origin", '
            f"{network_options(credentials=basic)})"
        )
        assert "secret" not in basic_result
        assert "secret" not in (Path(directory) / "auth-basic.git" / "config").read_text()
        bearer = 'some({username: none, password: none, bearer-token: some("secret")})'
        ok(
            f'clone("/repos/auth-bearer.git", {string(auth_url)}, "origin", '
            f"{network_options(credentials=bearer)})"
        )
        auth_server.seen_authorization.clear()
        server.fault = "redirect"
        server.redirect_url = auth_url
        redirected = invoke(
            f'clone("/repos/redirect.git", {string(base_url + "/source.git")}, "origin", '
            f"{network_options(credentials=basic)})"
        )
        assert redirected.startswith("err(network(http-status(302)))"), redirected
        assert auth_server.seen_authorization == [], auth_server.seen_authorization
        server.fault = None
        auth_server.shutdown()
        auth_server.server_close()
        auth_thread.join(timeout=5)

        # HTTP status, bounded response, malformed packet, and corrupt pack errors.
        server.fault = "http-error"
        status_error = invoke(clone_expr("http-error.git"))
        assert status_error.startswith("err(network(http-status(500)))"), status_error
        server.fault = "oversized"
        oversized = invoke(
            clone_expr("oversized.git", opts=network_options(response=128, pack=128))
        )
        assert oversized.startswith("err(network(response-too-large))"), oversized
        server.fault = "malformed"
        malformed = invoke(clone_expr("malformed.git"))
        assert malformed.startswith("err(network(malformed-response("), malformed
        server.fault = "bad-pack"
        corrupt = invoke(clone_expr("corrupt-pack.git"))
        assert corrupt.startswith("err(network(malformed-response("), corrupt
        assert "checksum" in corrupt.lower(), corrupt
        server.fault = None

        # A second clone snapshots the original remote-tracking refs for a stale-CAS test.
        ok(clone_expr("stale.git"))
        run("git", "-C", str(worktree), "switch", "--quiet", "main")
        (worktree / "readme.txt").write_text("second\n")
        run("git", "-C", str(worktree), "commit", "--quiet", "-am", "Second")
        run("git", "-C", str(worktree), "push", "--quiet", "origin", "main")
        run("git", "-C", str(worktree), "tag", "-a", "v2.0", "-m", "Release two")
        run("git", "-C", str(worktree), "push", "--quiet", "origin", "v2.0")
        second = run("git", "-C", str(worktree), "rev-parse", "HEAD")

        # Change a tracking ref while upload-pack is in flight; CAS must reject it.
        server.pause_post = True
        server.post_received.clear()
        server.release_post.clear()
        fetch_output = []

        def stale_fetch():
            fetch_output.append(invoke(fetch_expr("stale.git")))

        fetch_thread = threading.Thread(target=stale_fetch)
        fetch_thread.start()
        assert server.post_received.wait(20), "fetch never reached upload-pack"
        git_dir("stale.git", "update-ref", "refs/remotes/origin/main", feature, first)
        server.release_post.set()
        fetch_thread.join(timeout=30)
        assert not fetch_thread.is_alive(), "stale fetch did not finish"
        assert fetch_output and fetch_output[0].startswith("err(conflict("), fetch_output
        assert git_dir("stale.git", "rev-parse", "refs/remotes/origin/main") == feature

        # Normal fetch updates remote-tracking refs but not local branches or tags.
        fetched = ok(fetch_expr("network.git"))
        assert "refs/remotes/origin/main" in fetched, fetched
        assert git_dir("network.git", "rev-parse", "refs/remotes/origin/main") == second
        assert git_dir("network.git", "rev-parse", "refs/heads/main") == first
        assert git_dir("network.git", "rev-parse", "refs/tags/v1.0") == feature
        assert git_dir("network.git", "rev-parse", "refs/tags/v2.0") == git_dir(
            remote, "rev-parse", "refs/tags/v2.0"
        )
        git_dir("network.git", "fsck", "--strict", "--full")

        # The component refuses non-bare repositories rather than risking dirty files.
        dirty_worktree = Path(directory) / "dirty-worktree"
        run("git", "clone", "--quiet", str(remote), str(dirty_worktree))
        (dirty_worktree / "readme.txt").write_text("local dirty change\n")
        dirty_fetch = invoke(
            f'fetch("/repos/dirty-worktree", "origin", {network_options()})'
        )
        assert dirty_fetch.startswith("err(unsupported("), dirty_fetch
        assert (dirty_worktree / "readme.txt").read_text() == "local dirty change\n"

        # Held locks survive a failed fetch and prevent the remote-tracking update.
        (worktree / "readme.txt").write_text("third\n")
        run("git", "-C", str(worktree), "commit", "--quiet", "-am", "Third")
        run("git", "-C", str(worktree), "push", "--quiet", "origin", "main")
        third = run("git", "-C", str(worktree), "rev-parse", "HEAD")
        lock = cloned_repo / "refs/remotes/origin/main.lock"
        lock.write_text("held by another writer")
        lock_error = invoke(fetch_expr("network.git"))
        assert lock_error.startswith("err(conflict("), lock_error
        assert lock.read_text() == "held by another writer"
        assert git_dir("network.git", "rev-parse", "refs/remotes/origin/main") == second
        lock.unlink()
        git_dir("network.git", "fsck", "--strict", "--full")
    finally:
        server.release_post.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    run("wasm-tools", "validate", "--features", "all", str(WASM))
    wit = run("wasm-tools", "component", "wit", str(WASM))
    imports = re.findall(r"^\s*import ([^;]+);", wit, re.MULTILINE)
    assert imports and all(name.startswith("wasi:") for name in imports), imports
    assert any(name.startswith("wasi:http/outgoing-handler") for name in imports), imports
    assert "export yoshuawuyts:git/repository@0.1.0;" in wit

    # Keep fixtures under the worktree, including Windows CI runs.
    with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
        repo = Path(directory) / "agent.git"

        def invoke(expression, granted=True):
            args = [WASMTIME, "run", "-S", "http"]
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

        exercise_network_component(directory, invoke, ok, error, native_at)

    print(
        "Git component: Wasm clone/fetch, packs, refs, patches/blame, merge/rebase, "
        "CAS writes, and Git interoperability passed."
    )


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.stderr.write(error.stderr)
        raise
