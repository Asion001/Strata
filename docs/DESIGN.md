# Strata design (summary)

Strata is a self-hosted file cloud. It borrows ideas from Nextcloud but shares
no code with it and offers no Nextcloud API compatibility (yet).

## Stack

* **Server**: Rust, axum on tokio, sqlx with PostgreSQL (SQLite may follow),
  Apache OpenDAL for storage backends, BLAKE3 hashing, FastCDC chunking.
* **Web UI**: SvelteKit + TypeScript, built as a static single-page app, served
  by the Rust binary and installable as a PWA.
* **Workspace**: `crates/server` (HTTP API), `crates/storage` (connections,
  hashed store, chunker), `crates/sync` (placeholder for the sync client),
  `web/`, `docs/`.

## Storage model

### Connections
A connection is a storage backend (local folder, S3, FTP to start; all through
OpenDAL) plus credentials. The config is stored as JSON encrypted with
XChaCha20-Poly1305 (key derived from `STRATA_SECRET_KEY`). Each connection
has a `capabilities` JSON column, e.g.

```json
{ "snapshots": "none|btrfs|zfs|s3_versioning|smb", "snapshots_create": false,
  "rename": true, "range_read": true, "fs_type": "btrfs" }
```

For local connections the filesystem type is detected with `statfs(2)` when the
connection is added, so phase 2 can use native btrfs/ZFS snapshots (falling
back to Strata's own history elsewhere). Local folders always live below
`STRATA_LOCAL_BASE` and only admins may create them. A background job probes
every connection's health every 10 minutes.

### Volumes and mounts
A **volume** is `(connection, folder inside it, mode)`. A **mount** shows a
volume at a folder of a user's tree: `(owner, path, volume)`. Each user has a
private root mount (`path = ''`) on the server's default storage, created with
the user. A user can mount any connection they own at any folder, including
inside another mount, to any depth. A mount point shadows a real folder with
the same name on its parent. Removing a mount never deletes data: remounting
the same connection folder in the same mode brings the tree back.

Modes, freely nestable inside each other:

* **RAW**: backend folders match the tree. Listings are cached in `nodes`;
  a folder is re-listed from the backend when its cache is older than
  `STRATA_RAW_TTL_SECS` (default 30 s) or on a full rescan, which picks up
  changes made outside Strata.
* **HASHED**: content-addressed blobs named by their BLAKE3 hash, sharded as
  `blobs/ab/cd/<hash>` under the volume folder. Files up to 4 MiB are one
  blob; larger files are split with FastCDC (min 512 KiB, avg 2 MiB, max
  8 MiB). The tree lives only in PostgreSQL (`nodes.chunks`,
  `nodes.chunk_sizes`); a file's identity is BLAKE3 over its chunk hashes.

Moves inside one hashed volume only update the database. Moves inside one raw
volume use the backend's rename (a real `rename(2)` for local folders) or copy
plus delete where there is none (S3). Moves across volumes copy, verify the
destination's BLAKE3 against the source stream, then delete the source.
Moving a mount point moves the mount itself.

### Change journal
Every tree change is recorded once, in the journal of its storage location
(volume), so a folder seen by several users is stored once. Each volume (and
each user's own tree, for mount points added, moved or removed) has a gap-free
sequence allocated under a row lock in the inserting transaction, so sequence
order is commit order. A user's feed (`GET /api/changes?cursor=TOKEN`, at most
1,000 entries a page) merges the journals of the volumes they mount, mapped into
each mount's path, with their own mount events. The cursor is an opaque token
of scope -> seq; scopes the user cannot see are ignored. Without a cursor the
endpoint returns the current position. Entries older than 30 days (or beyond
1M per scope) are pruned; an older cursor gets `410` with `"resync": true`.
Raw volumes on the same connection whose folders overlap both get a change
(their listing cache is marked stale), and the UI warns when such an overlap is
created. `GET /api/ws` pushes the newest cursor; instances learn about changes
through PostgreSQL `LISTEN/NOTIFY`, carrying ids only.

### Garbage collection
Blobs no file and no pending upload reference are deleted after a grace
period (`STRATA_GC_GRACE_HOURS`, default 24), in two steps: mark the row
`deleting`, then (holding the row lock, re-checking) delete the object and the
row. A `blobs` row is only inserted after its object is written; a writer that
meets a `deleting` row rewrites the object and clears the flag, so a crash at
any point never leaves a live row without its object. Re-using a blob bumps its
age. One unreachable backend never stops GC elsewhere.

### Write ordering
Tree changes of one volume (including raw backend renames, the atomic close of
a raw write, and raw rescans) run under a per-volume lock: an in-process queue
in front of a PostgreSQL advisory lock, so journal order matches the order of
the changes, even across server instances. Cross-volume moves copy and verify
each file, then delete the source only if it is unchanged; folders are removed
only once empty, so an entry that cannot be copied is never lost. Local writes
go to a temp folder inside the connection and are renamed into place.

## Uploads
Resumable and chunked. The browser chunks with the same FastCDC parameters
and gear table as the server (a parity test keeps them identical), hashes
chunks with BLAKE3 (WASM), and asks which chunks it still has to send. For
hashed volumes the answer only skips chunks the *same user* stored before, so
it never reveals whether another user has a file; cross-user deduplication
happens server-side (an already stored blob is not written twice). Raw volumes
do not negotiate: every chunk is sent, staged per upload, and streamed to the
backend on commit.

Blob names are the plain BLAKE3 hash today. The store's naming function
already accepts a per-connection key (BLAKE3 keyed mode) for when encryption
at rest arrives, so the backend will not learn content hashes.

## Text editor
Text, Markdown and code open in a CodeMirror 6 editor that loads lazily,
with language modes loaded per file extension. Markdown gets a live preview
(markdown-it with raw HTML off, then DOMPurify).

Saves never overwrite silently:
- `PUT /api/files/content?path=` takes `If-Match` with the ETag from the
  GET. A changed file gets 412 with its current ETag. A missing precondition
  on a write gets 428, and creating a file takes `If-None-Match: *`.
- GET and PUT share one version function: hashed volumes use the content
  hash; raw volumes hash the size and the backend etag (or the nanosecond
  mtime where the backend has no etag).
- Raw metadata can be coarse (whole-second mtimes), so the editor also sends
  `X-Strata-Base-Hash`, the BLAKE3 of the bytes its edit is based on, and the
  server compares it with the current content.
- A cheap metadata pre-check runs before anything is written. The full check
  runs under the volume lock in the same critical section as the write
  (raw: right before the atomic close, aborting on mismatch).
- Writes take the normal store paths, so the journal and websocket push fire.

On a 412, the editor offers three choices: keep mine (a copy named
`name (conflict YYYY-MM-DD HH-MM).ext`), reload theirs, or view a side-by-side
diff. Unsaved drafts autosave to IndexedDB every 2 s, keyed by user, mount and
path with their base ETag and hash, and are offered back on reopen. They
expire after 30 days, are cleared on sign-out, and other users' drafts are
dropped when someone signs in. Line endings and
the trailing newline are preserved; mixed-ending files are kept verbatim.
Files over 5 MB or not valid UTF-8 open read-only, files over 20 MB are not
loaded, and files with NUL bytes in their first 8 KB are refused.

## Security notes
* Argon2id passwords; sessions are random tokens stored as BLAKE3 hashes.
* Cookie sessions require an `X-Strata` header on mutating requests (CSRF).
* Uploaded files never run script in the app (`crates/server/src/safe.rs`).
  Every response with user file bytes (content, downloads, the image-preview
  route, thumbnails, and the share-link versions of all of them) sends
  `Content-Security-Policy: sandbox; default-src 'none'; ...`, `nosniff`,
  `Cross-Origin-Resource-Policy: same-origin` and `Referrer-Policy: no-referrer`.
  The Content-Type comes from an allowlist keyed on magic bytes, never from the
  extension alone: anything else is `text/plain` (with `charset=utf-8` or
  `utf-16le/be` when known; legacy 8-bit text has none) if it looks like text,
  or `application/octet-stream` as an attachment. HTML, SVG, XML, JS and CSS
  are therefore served as plain text. Sync-word-only audio (MP3/ADTS without
  ID3) never wins over text. SVG is `image/svg+xml` only on the image-preview
  routes (`/files/image`, `/public/{token}/image`), which the UI loads only
  through `<img>`. Types are decided from a complete head and cached per file
  version. Content-Disposition
  uses RFC 6266 `filename*` with control characters and quotes stripped.
* The app's own pages have a strict CSP (scripts by hash, as the static SPA
  has no per-request HTML for nonces; no `unsafe-eval`, only
  `wasm-unsafe-eval` for the BLAKE3 WebAssembly).
* Share links may have a password (Argon2) and an expiry.

## Later phases
Native snapshots and versions, sync client (`crates/sync`), WebDAV, video/PDF
thumbnails, sharing between users, SQLite.
