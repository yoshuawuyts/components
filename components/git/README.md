# git

A **self-contained Git client for agents**, implemented with
[gitoxide](https://github.com/GitoxideLabs/gitoxide) and compiled to a callable
WebAssembly component. Git objects, trees, commits, revision parsing and
packfile decoding run **inside Wasm**. There is no host Git executable,
subprocess import, libgit2, shell, or custom host service.

This first version is a **local, bare-repository client**, not a complete
replacement for the Git CLI. It supports SHA-1 repositories:

| Operation | Behavior |
| --- | --- |
| `init` | Create a new bare repository with an explicit initial branch |
| `resolve` | Resolve revision expressions such as `HEAD`, `main~2`, or object IDs |
| `references` | List sorted refs and peel annotated tags |
| `log` | Read bounded first-parent history with raw commit messages |
| `list-tree` | List sorted recursive leaf entries, including modes and gitlinks |
| `read-file` | Read binary blobs or symlink target bytes at a revision |
| `diff` | Compare two trees by path, object ID and file mode |
| `create-branch` | Create a branch without replacing an existing ref |
| `commit-files` | Commit explicit additions, replacements and deletions with optimistic concurrency |

Reads support loose and packed objects, packed references, ordinary working
repositories and bare repositories. Both mutation operations require a **bare
repository**: we do not bypass a working repository's index or silently leave
its working tree inconsistent.

**Not implemented:** network clone/fetch/push, SSH, credentials, worktree
checkout/status, staging, merge/rebase, text patches, rename detection, Git LFS,
SHA-256 repositories, signing, hooks, filters, and reflogs. Submodule entries can
be inspected but their repositories are not traversed. Ref names, file paths,
and identities must be UTF-8; binary file contents and messages are preserved.
Colon-leading revision expressions (including index lookups such as `:file`)
are explicitly rejected; tree lookups such as `HEAD:file` are supported.
There is deliberately no arbitrary command or shell escape export.

## Build and use

From the repository root:

```sh
rustup target add wasm32-wasip2
cargo build -p git --release --target wasm32-wasip2
wasm-tools validate --features all target/wasm32-wasip2/release/git.wasm
```

The artifact is `target/wasm32-wasip2/release/git.wasm`. The contract is
[`wit/world.wit`](wit/world.wit), exporting
`yoshuawuyts:git/repository@0.1.0`. Sync local filesystem operations use WASI
0.2; they require no async runtime or WASI HTTP implementation.

For a quick Wasmtime example, grant a parent directory and create the repository
inside it. Use a fresh directory name; `init` rejects existing destinations.

```sh
mkdir -p target/agent-repositories
wasmtime run --dir target/agent-repositories::/repos \
  --invoke 'init("/repos/example.git", "main")' \
  target/wasm32-wasip2/release/git.wasm

wasmtime run --dir target/agent-repositories::/repos \
  --invoke 'commit-files("/repos/example.git", "main", none, [{path: "hello.txt", contents: some([104, 105, 10]), mode: regular}], {name: "Agent", email: "agent@example.invalid", seconds: 1700000000, offset: 0}, "Initial commit")' \
  target/wasm32-wasip2/release/git.wasm

wasmtime run --dir target/agent-repositories::/repos \
  --invoke 'log("/repos/example.git", "HEAD", 10)' \
  target/wasm32-wasip2/release/git.wasm
```

For an agent runtime, generate bindings from the WIT world and link standard
WASI imports. Preopen only the intended repository directory (or its parent for
`init`). Do not inherit environment variables or grant network access.
Read-only agents should receive read-only filesystem capabilities from the
embedding host; the component itself does not elevate access.

Every function returns `result<_, error>`. A Wasmtime CLI invocation returning
`err(...)` is an **application failure even when Wasmtime exits with status 0**;
hosts must inspect the result rather than just the process exit code.

## Committing safely

Call `resolve(path, branch)` and supply the returned full object ID as
`expected-parent`. For an initial commit, use `none`. `commit-files` inherits
unchanged files from that commit, applies the explicit changes and updates only
the requested branch. Author and committer both use the supplied signature;
`seconds` is Unix time and `offset` is seconds east of UTC, in whole minutes.

The branch is rechecked **under an exclusive Git-compatible `.lock` file** and
updated by atomic rename. A stale parent or held lock returns `conflict`; reread
the branch before retrying. Existing locks are never removed. Hooks and
external filters are not run. No reflog is written. Committing to another branch
does not change symbolic `HEAD`.

Duplicate/overlapping paths, traversal paths, `.git` components, nonexistent
deletions, replacing directories, and no-op commits are rejected. Deleting the
last file is supported. File modes are regular, executable, and symlink.
Symlinks are Git blobs containing target bytes, not host filesystem links.

Failed writes can leave unreachable Git objects, just as ordinary Git does;
they do not advance the branch. An interrupted write can leave a `.lock` file;
recovery is an explicit host/operator decision. Failed initialization may leave
a partial destination for inspection. Ref updates are atomic, but this version
does not promise power-loss durability of containing directories.

## WASI compatibility and limits

Gitoxide's readers use `memmap2`, but WASI has no memory-mapping API. The
workspace includes a small [compatibility adapter](../../support/memmap2) that
uses bounded byte-buffer reads on WASI and native mappings on native targets.
Keep the workspace's `[patch.crates-io]` setting when building this component
elsewhere; unpatched gitoxide can compile for WASI but fail when reading objects.
Gitoxide's process-ID-dependent ref locks are replaced by the lock protocol
above, without introducing a process capability.

History is limited to 1,000 commits per call. Tree/ref listings are limited to
100,000 entries; tree paths to 128 components. File reads and the total supplied
contents of one commit are limited to 16 MiB; commits accept at most 1,000
changes. Each buffered file mapping (including a packfile) is limited to
256 MiB. This is not a total memory or decompression limit: the embedding host
must also impose memory limits, fuel/epoch deadlines and filesystem quotas,
especially for untrusted repositories.

## Tests

```sh
cargo test -p git --lib
just test-git
```

`test-git` builds and validates the real component, executes it in Wasmtime, and
uses native Git **only in the test harness** as a fixture builder and format
oracle. It covers binary files, file modes, branch conflicts, held locks, denied
filesystem access, packed objects/refs, commit graphs, writes after packing, and
`git fsck --strict` interoperability.
