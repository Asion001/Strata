// Prints the browser chunker's chunk lengths for a file, as JSON. The Rust
// property tests (crates/storage/tests/properties.rs) compare them with the
// server's: node --experimental-strip-types scripts/chunk-lengths.ts <file>
import { readFileSync } from 'node:fs';
import { chunkLengths } from '../src/lib/chunker.ts';

const data = new Uint8Array(readFileSync(process.argv[2]));
process.stdout.write(JSON.stringify(chunkLengths(data)));
