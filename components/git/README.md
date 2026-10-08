# git

A **self-contained Git client for agents**, implemented with
[gitoxide](https://github.com/GitoxideLabs/gitoxide) and compiled to a callable
WebAssembly component. Git objects, trees, commits, revision parsing and
packfile decoding run **inside Wasm**. There is no host Git executable,
subprocess import, libgit2, shell, or custom host service.

This version is a **local Git client**, not a complete replacement for the Git
CLI. It supports SHA-1 bare and working repositories:

| Operation | Behavior |
| --- | --- |
| `init` | Create a new bare repository with an explicit initial branch |
| `resolve` | Resolve revision expressions such as `HEAD`, `main~2`, or object IDs |
| `references` | List sorted refs and peel annotated tags |
| `log` | Read bounded first-parent history with raw commit messages |
| `list-tree` | List sorted recursive leaf entries, including modes and gitlinks |
| `read-file` | Read binary blobs or symlink target bytes at a revision |
| `diff` | Compare two trees by path, object ID and file mode |
| `diff-renames` | Compare trees with deterministic exact-content rename pairing |
| `unified-diff` | Emit bounded Git-format textual patches with modes and rename metadata |
| `blame` | Attribute a bounded line range through linear history and clean renames |
| `create-branch` | Create a branch without replacing an existing ref |
| `commit-files` | Commit explicit additions, replacements and deletions with optimistic concurrency |
| `merge-branch` | Fast-forward or three-way merge branches with conflict-path reporting |
| `rebase-branch` | Replay bounded linear history with an explicit committer signature |
| `checkout` | Switch a working repository to a branch or commit |
| `status` | Report staged, unstaged, and non-ignored untracked paths |
| `add` / `remove` / `reset` | Update the working repository's index |
| `commit` | Commit the index and compare-and-swap the checked-out branch |

Reads support loose and packed objects, packed references, ordinary working
repositories and bare repositories. `commit-files` and `create-branch` are
bare-repository operations; working-tree operations require a non-bare
repository. Working-tree commits use the index and update only the checked-out
local branch, with an expected-parent compare-and-swap.

`merge-branch` and `rebase-branch` write only bare repositories. They never
change a working tree or index, and reject non-bare repositories rather than
risk staged, unstaged, untracked, or ignored work. Both operations require
expected full object IDs and update the target branch with the same lock-file,
recheck, and atomic-rename compare-and-swap used by commits. A stale tip fails
without updating a reference. A failed or interrupted operation may leave
unreachable objects, but never points a branch at a partial result.

Merges fast-forward when possible and otherwise use gitoxide's three-way tree
merge. Each input history is bounded to 1,000 reachable commits. Unresolved
conflicts return their repository-relative paths and do not advance the target
branch. The supplied signature is used as both author and committer for a merge
commit. Rebases replay at most 1,000 non-merge commits, preserving each original
author and raw message while using the supplied committer signature for every
rewritten commit. An unresolved replay returns the original commit ID and
conflicting paths; the branch remains at its original tip. Merge and rebase
reject `.gitattributes`, configured attribute files, external merge drivers,
external filters, and submodule entries rather than invoking or emulating them.
They do not run hooks or write reflogs.

**Not implemented:** network clone/fetch/push, SSH, credentials, patch application,
similarity-based rename detection, Git LFS, SHA-256 repositories, signing, hooks,
filters, and reflogs. Checkout does not recurse into submodules; sparse checkout
and split indexes are unsupported. On WASI, executable permission bits cannot
be set on checked-out files, so checking out an executable entry may appear as
a mode change to native Git. Ref names, file paths, and identities must be
UTF-8; binary file contents and messages are preserved.
Colon-leading revision expressions (including index lookups such as `:file`)
are explicitly rejected; tree lookups such as `HEAD:file` are supported.
There is deliberately no arbitrary command or shell escape export.

## Patches, renames, and blame

The original `diff` API remains a path-by-path tree comparison, with no rename
inference. `diff-renames` pairs deleted and added paths with identical blob IDs
and compatible file types (regular/executable files are compatible). Multiple
identical candidates are paired deterministically in path order. This is
**exact-content detection only**, not Git's similarity scoring: edited renames,
copies, and renames that replace an existing path are reported as ordinary
changes. Each result includes old/new entries (IDs, paths and modes) and an
explicit `renamed` flag.

`unified-diff` accepts `patch-options` with `context-lines` (0..=100),
`max-files` (1..=1000), `max-bytes` (1..=16,777,216), and `detect-renames`.
It returns one `file-patch` per change, including the same typed metadata,
a `binary` flag, and patch bytes. Concatenate the byte arrays in result order
to produce a Git-format patch. Text need not be UTF-8. Paths are C-quoted
where necessary, modes and empty file additions/deletions are represented in
headers, and unterminated lines carry `\ No newline at end of file` markers.
Regular-file/symlink type changes emit delete/add sections. Patches with zero
context require Git's `apply --unidiff-zero` option, just like `git diff -U0`.

NUL-containing blobs are explicitly marked binary (the entire blob is scanned).
Changed binary contents produce only `Binary files ... differ` diagnostics,
**not applicable binary deltas**. Binary content additions/deletions are likewise
diagnostic-only; exact-content binary renames and mode-only changes still have
applicable metadata. Submodules and special modes return `unsupported`.
Diffing uses raw stored blobs and gitoxide's in-memory histogram/slider diff:
attributes, textconv, configured diff drivers and external filters are never
invoked or emulated. These patches intentionally describe object bytes rather
than filtered working-tree bytes.

`blame` accepts `blame-options` with one-based `start-line`, `max-lines`
(1..=100,000), and `max-commits` (1..=1000). Each returned line contains its
requested line number, introducing commit ID, and original path/line number.
The requested range is clipped to EOF; line 1 of an empty regular file returns
an empty list, while other out-of-range starts fail. Unchanged lines are mapped
back through line edits and unambiguous exact-content renames, including mode
changes. Binary files, symlinks, merge traversal, and ambiguous rename origins
return `unsupported`. Blame does not infer edited renames, copies, or moved
blocks, and treats line terminators as part of a line's content. It is not an
implementation of Git's `-M`, `-C`, whitespace-ignore or merge blame heuristics.
Root lines retain the root commit ID (no synthetic boundary ID).

All these operations fail the whole request rather than returning truncated
patches or fabricated boundary attribution when a bound is exceeded. Patch
and blame inputs are limited to 16 MiB per loaded blob and 64 MiB total loaded
blob bytes per call; textual blobs are limited to 100,000 lines. Blame also
limits the sum of visited tree leaf entries to 1,000,000. The history bound
counts the tip and inspected parents; unresolved attribution fails at the
boundary. Object decompression still needs host memory/fuel limits as described
below. Read-only patch/blame paths support Git-valid UTF-8 names including
tabs, newlines, quotes, backslashes and colons; write APIs retain their portable
path restrictions.

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

Branch merge and rebase use the same compare-and-swap ref update. Merge requires
both observed branch tips; rebase requires the observed branch tip and an exact
commit ID for its new base. Neither operation accepts an implicit `HEAD` or
updates a working repository's index or files.

Duplicate/overlapping paths, traversal paths, `.git` components, nonexistent
deletions, replacing directories, and no-op commits are rejected. Deleting the
last file is supported. `add`, `remove`, and `reset` require explicit paths and
do not implement Git pathspec magic. Symlinks are created as filesystem links
during checkout and represented in the index as Git blobs containing target
bytes.

Checkout refuses staged or unstaged changes and untracked-file collisions by
default. `force: true` permits overwriting tracked changes and colliding
untracked paths; unrelated untracked files are preserved. Worktree operations
do not run hooks or filters, so repositories that rely on clean/smudge filters
or LFS need those transformations performed externally.

Index reads obtain the index timestamp through
`std::fs::Metadata::modified()` rather than gitoxide's unsupported WASI
filetime conversion. Unmerged entries, split indexes, sparse indexes, and
special index flags are rejected rather than silently rewritten.

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
oracle. It covers binary files, file modes, branch conflicts, merge and rebase
success/conflicts/stale tips, held locks, denied filesystem access, packed
objects/refs, commit graphs, writes after packing, and `git fsck --strict`
interoperability.
It also feeds the exact Wasm-emitted patch bytes to native `git apply --index`
and compares resulting trees, covering clean renames, mode/type changes,
binary diagnostics, empty files, raw non-UTF-8 text, unusual quoted paths,
multiple hunks, context bounds, and EOF markers. Linear-history blame and clean
rename attribution are compared with native `git blame --line-porcelain`,
including packed-object reads and traversal/range limits.
