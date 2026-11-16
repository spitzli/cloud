// Writes .br/.gz siblings for hashed static assets; Core's ServeDir serves them by Accept-Encoding.
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { brotliCompressSync, gzipSync, constants } from 'node:zlib';

const dir = fileURLToPath(new URL('../dist/assets/', import.meta.url));
const compressible = /\.(js|mjs|css|svg|json|wasm|txt|map)$/;
let count = 0;
for (const name of await readdir(dir)) {
  if (!compressible.test(name)) continue;
  const path = join(dir, name);
  const raw = await readFile(path);
  if (raw.length < 1024) continue;
  const br = brotliCompressSync(raw, { params: { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_SIZE_HINT]: raw.length } });
  const gz = gzipSync(raw, { level: 9 });
  if (br.length < raw.length) await writeFile(`${path}.br`, br);
  if (gz.length < raw.length) await writeFile(`${path}.gz`, gz);
  count++;
}
console.log(`precompressed ${count} assets`);
