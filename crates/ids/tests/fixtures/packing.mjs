// Independently expressed arithmetic oracle. IDs remain BigInt/decimal strings.
import { readFileSync } from 'node:fs';
const epoch = 1767225600000n;
const cases = [
  [0n, 0n, 2n, 0n], [1n, 0n, 2n, 0n], [1n, 1n, 0n, 4095n],
  [123456789n, 17n, 7n, 83n], [2199023255551n, 63n, 15n, 4095n],
];
const fixture = cases.map(([elapsed, machine, generator, sequence]) => ({
  timestamp_ms: Number(epoch + elapsed), machine: Number(machine), generator: Number(generator), sequence: Number(sequence),
  id: (elapsed * 4194304n + machine * 65536n + generator * 4096n + sequence).toString(),
}));
const expected = JSON.stringify(fixture, null, 2) + '\n';
if (process.argv.includes('--check')) {
  if (readFileSync(new URL('./packing.json', import.meta.url), 'utf8') !== expected) throw new Error('BigInt fixtures differ');
} else { process.stdout.write(expected); }
