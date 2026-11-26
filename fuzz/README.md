# Fuzz targets

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets (libFuzzer) that call the server's own code:

| Target | What it checks |
|---|---|
| `path_normalize` | `paths::normalize` on any input: the result never escapes a base folder, has no `.`, `..`, empty or control-character component, and normalizing it again changes nothing. `split`, `join` and `strip_prefix` agree on it. |
| `range_header` | `Range` and `If-Range` against any file size: a part is never empty and stays inside `0..size`; If-Range lets a range through only for the exact ETag. |
| `upload_protocol` | Random sequences of upload create, status, chunk put and commit, through the real router against Postgres, on a hashed and a raw volume. No 5xx, a chunk is never taken under another hash, and a committed file is exactly its declared chunks. |
| `token_decode` | Share tokens and change-feed cursors: no panic, malformed input refused, decoded cursors round-trip. |
| `content_disposition` | The `Content-Disposition` header for any file name: no CR, LF or NUL, and nothing in the quoted name can end it early. |

Run one (nightly Rust; `upload_protocol` also needs `STRATA_TEST_DATABASE_URL`, a Postgres it may create a database in):

```sh
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz run -O path_normalize corpus/path_normalize -- -max_total_time=60
```

CI runs each target for 2 minutes on every pull request, and for an hour every night. The seed corpora are in `corpus/`. After a long run, fold new inputs back in with `cargo +nightly fuzz cmin -O <target> corpus/<target>`.

A crash is saved under `artifacts/<target>/`. Reproduce it with `cargo +nightly fuzz run -O <target> <file>`, then turn it into an ordinary unit or API test before fixing it.
