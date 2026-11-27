// Prints a file's BLAKE3 (hex), as the app hashes upload chunks. Used by
// the load test (scripts/load-test.sh): node --experimental-strip-types scripts/blake3-hex.ts <file>
import { readFileSync } from 'node:fs';
import { blake3 } from 'hash-wasm';

process.stdout.write(await blake3(new Uint8Array(readFileSync(process.argv[2]))));
