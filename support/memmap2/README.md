# WASI read-only mapping adapter

This unpublished workspace patch supplies the read-only `memmap2` subset used
by gitoxide. WASI reads files into owned buffers instead of calling unsupported
memory-mapping syscalls. Reads are capped at 256 MiB, preserve the file cursor,
and report short reads, bad ranges, allocation failures, and restoration errors.
No unsafe code is used by the WASI implementation.

Native targets reexport upstream `memmap2` 0.9.11 at its exact published Git
revision, preserving the native API and behavior. The distinct Git source
prevents a recursive dependency on this crates.io compatibility patch.
This adapter is not a general
replacement for memmap2 on WASI: writable/anonymous mappings and platform
advice APIs are not provided there. Do not publish it as an upstream package.
