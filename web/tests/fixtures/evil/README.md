Hostile files for tests: none of them may ever run script in the app.

- `evil.html`: inline script and an `onerror` handler.
- `evil.svg`: `onload` and an embedded `<script>`.
- `polyglot.jpg` / `polyglot.html`: a JPEG whose comment segment holds an HTML page.

The file name with a quote, CR LF and a fake `Set-Cookie` header is created
at runtime by the tests: such names can't be committed portably, and the API
refuses control characters in names anyway.
