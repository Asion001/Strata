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
has a `capabilities` JSON column, detected when it is added and again on
every health probe:

```json
{ "snapshots": { "kind": "none|btrfs|zfs|s3_versioning|smb", "create": false },
  "rename": true, "range_read": true, "atomic_write": true, "list_fast": true,
  "fs_type": "btrfs" }
```

* **Local:** the filesystem comes from `statfs(2)` (btrfs `0x9123683E`, ZFS
  `0x2FC12FC1`; the type name on macOS). `snapshots.create` is set only when
  a real test works: a scratch subvolume and a (writable, so rmdir can remove
  it) snapshot of it on btrfs, both removed again; or the `snapshot` and
  `mount` permissions for the dataset in `zfs allow`.
* **S3:** `GetBucketVersioning` (a SigV4-signed request) turns on
  `s3_versioning`.

Local folders always live below `STRATA_LOCAL_BASE` and only admins may
create them.

### Connection health
Each connection is probed every `STRATA_PROBE_SECS` (default 60, up to 10%
less as jitter; half that while reads are failing), one probe per connection
at a time.
* **What a probe does.** It reads the root, then writes, reads back and
  deletes one small file, `probe-<random>.tmp`, under `.strata-probe/` at the
  connection's root. Manual checks (adding, editing, "Check") never count
  towards offline. A probe that started before an edit has its result
  dropped.
* **Local folders.** A local connection's folder is never created again
  once it exists (OpenDAL would, so no operator is built for a missing
  one). Its marker, `.strata-probe/conn-<id>`, is written when the folder
  is made (a folder from before markers existed is adopted by the first
  healthy probe, if it isn't empty). From then on a folder without the
  marker counts as missing. An unmounted disk leaves its empty mount point behind,
  which is a folder, but not this one. The folder and marker are checked
  before every use, off the async threads and with a time limit, and a
  missing one gets 503 at once rather than only after three probes.
  Requests that arrive together share one fresh check.
* **Cost.** A healthy connection's capability detection and write test
  run every ten minutes; the probes in between only read the root (every
  request is billed on paid object storage).
* **No junk files.** The probe file is deleted whatever happened (the
  marker is the only file ever left there). Files an
  interrupted probe left behind are swept by later probes once they are ten
  minutes old. Versioned buckets get no write test, since a delete there
  would leave versions behind.
* **Reserved folder.** `.strata-probe` is reserved at any depth: hidden
  from raw listings, not reachable through mounts, and refused in
  connection and mount paths.
* **States.**
  * `ok`: everything worked.
  * `degraded`: the write test failed, or reads failed fewer than three
    times in a row.
  * `offline`: three failed reads in a row. One successful read ends it.
* **What gets recorded.** The state, the last latency, a plain-words reason
  and the backend's message. Credentials, `user:password@` and URL query
  strings are scrubbed from the message. Only admins see the message; owners
  see the reason.
* **Offline connections.** The storage isn't touched at all: reads and writes
  through the connection's mounts get 503, including changes a hashed mount
  keeps only in the database. The mount points stay in listings (flagged
  `offline`), and hashed mounts still list from the database.
* **Live updates.** State changes are pushed over the websocket
  (`{"health": <connection id>}`) to users who mount the connection, its
  owner and admins.

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

### Trash
Deleting a file or folder moves it to its owner's trash (`trash` table), from
which it can be restored, purged, or emptied; items past
`STRATA_TRASH_DAYS` (default 30) are purged by the hourly background job.
`0` turns the trash off for new deletes; items already in the trash keep the
purge date they were given. `POST /api/files/delete` with `permanent: true` skips
the trash.

* **Hashed volumes:** the item's `nodes` rows move to `trash_nodes` in one
  transaction. GC counts `trash_nodes` as references, so blobs live until the
  purge; the backend is never touched.
* **Raw local folders:** the item is renamed to
  `<connection root>/.strata-trash/<id>/<name>` (hidden like `.strata-tmp`,
  and refused as a connection or mount base). The row is written with
  `ready = false` before the rename and marked ready after it; the background
  job reconciles rows a crash left behind (no item moved: drop the row; item
  moved: mark ready; item gone: drop the row), each under the volume lock.
* **Other raw backends (S3, FTP):** no cheap, safe rename, so deletes stay
  permanent; the listing's `mount.trash` says so, for the UI to warn.

A restore puts the item back at its old place in the volume, recreating
missing parent folders, under "name (restored).ext" if the name is taken. A
raw file is put back with a hard link (falling back to rename), so an entry
created outside Strata meanwhile is never replaced. Restoring needs a mount of
the same volume; trash rows journal as `delete`, restores as `create`. Share
links come back with an item restored to the same path (hashed rows keep their
ids; for raw items, links to the item itself). Trash on an unmounted disk is
never taken as empty: a missing item counts as gone only while the trash folder
or the connection's root marker is there. A connection with trashed items is
deleted only with `?purge_trash=true`, and one with raw trash can't be pointed
at another folder; deleting a user removes their trash. A purge renames the
item to `.strata-trash/purging-<id>` under the volume lock and removes it
after. The background job removes left-over `purging-*` folders and sets
`.strata-trash/<id>` folders no row knows aside as `orphan-<id>-<time>` (kept
for an admin, never deleted: a database restored from backup can leave real
files there).

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

## Downloads and media
File contents (`/api/files/content`, and the public share route) support a
single byte range (RFC 9110).
* **Range forms.** `bytes=a-b`, `a-` and `-n` get 206 with Content-Range,
  and a range past the end gets 416 with `Content-Range: bytes */size`.
  Several ranges, another unit or a malformed header get the whole file.
* **If-Range.** It lets a range through only on an exact match with the
  current ETag. A coarse raw version (no backend ETag, seconds-only mtime)
  never matches, since it can stay the same across a rewrite. Where the
  backend supports it (S3), a raw read is pinned to the version that was
  checked (If-Match), so a change mid-download fails the read instead of
  mixing two versions.
* **Accept-Ranges and HEAD.** Responses carry `Accept-Ranges: bytes`. HEAD
  gives size, type and ETag with no body.
* **Hashed mounts.** The range maps to the chunks covering it, using the
  stored chunk sizes. Only those are read, one at a time, with the first
  and last sliced; the file's first bytes are also read once for the type
  sniff.
* **Raw mounts.** The backend reads the range itself (local, S3 and FTP
  all can). The health check's write test also reads its test file from an
  offset; a backend where that returns the wrong bytes (`range_read:
  false`) is read in one plain read from the start, dropping the bytes before the range and stopping after it (logged
  once per connection).
* **Playback.** Opening a file starts playback, from where it stopped last
  time.
* **Memory.** Responses stream with backpressure, so memory per download
  is bounded: ten parallel 1 GB downloads stayed flat at about 200 MB.

Video and audio play in the native elements (`preload="metadata"`,
`playsinline`), so seeking is ranged reads. The preview viewer steps to the
next or previous file in the folder, and each file's playback position is
remembered in this browser (`localStorage`, the 200 most recent).

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
- Raw metadata can be coarse (whole-second mtimes), so on raw volumes an
  `If-Match` save must also send `X-Strata-Base-Hash`, the BLAKE3 of the bytes
  the edit is based on (428 without it). The server compares that hash with
  the current content. Coarse raw files are never answered with 304, and
  their type isn't cached.
- Hashed volumes check under the volume lock in the same critical section as
  the node write. Raw volumes take the volume lock, check, then write and
  close atomically while still holding it. A refused save therefore writes
  nothing at all (no temp files, no folders). Saves are at most 5 MB.
- Writes take the normal store paths, so the journal and websocket push fire.

On a 412, the editor offers three choices: keep mine (a copy named
`name (conflict YYYY-MM-DD HH-MM).ext`), reload theirs, or view a side-by-side
diff. Unsaved drafts autosave to IndexedDB every 2 s, keyed by user, mount and
path with their base ETag and hash, and are offered back on reopen. They
expire after 30 days, are cleared on sign-out, and other users' drafts are
dropped when someone signs in. Autosave only writes while the draft's user is
still the one signed in (a shared marker every tab sees). Line endings and
the trailing newline are preserved; mixed-ending files are kept verbatim.
Files over 5 MB or not valid UTF-8 open read-only, files over 20 MB are not
loaded, and files with NUL bytes in their first 8 KB are refused.


## PDF viewer
PDFs open in pdf.js (`pdfjs-dist`, pinned to an exact version) inside the
preview viewer. Both the preview viewer and pdf.js load only when a file is
opened, so neither is in the startup bundle (`npm run size` checks this in
CI against `web/bundle-baseline.json`).
* **Range loading.** The file is read from the normal content route with
  byte ranges. Streaming and background fetching are off and chunks are
  256 KB, so a large file shows its first page after a few small reads.
  File content responses are never compressed on the fly, since that would
  drop the Content-Length that range readers need.
* **Same origin.** The worker is a build asset, and CMaps, standard fonts,
  WASM decoders and ICC profiles are copied to `/pdfjs/<version>/` at build.
  pdf.js 6 compiles no code at runtime, so it runs under the app's CSP with
  no extra allowances. The worker, stylesheet and data files are fetched only
  when a PDF opens; the service worker doesn't precache them.
* **No document scripting.** No scripting manager is attached, forms and
  XFA are off (annotations are links only), and links leave the viewer only
  for http(s) URLs, in a new tab with `noopener noreferrer`.
* **Viewer features.** Thumbnails render as they scroll into view. Find uses
  pdf.js's find controller. Print renders the pages to images and prints
  those.
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
