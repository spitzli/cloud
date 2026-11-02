// Development-time parity capture; the Rust service never launches JavaScript.
// Run from the workspace root after an intentional retained contract change.
import { z } from 'zod';
import { writeFileSync } from 'node:fs';
import { commandSchema, observationSchema, appSpecSchema } from '../../../packages/hosting-contracts/src/index.ts';
const supported = commandSchema.options.filter(option => /^(?:apps\.|clusters\.|addresses\.|limits\.|usage\.get$|projects\.(?:get|list|create|update)$|operations\.(?:get|overview)$)/.test(option.shape.action.value));
const hostingCommands = z.discriminatedUnion('action', supported);
const only = process.argv.slice(2);
if (only.some(name => !['legacy-command', 'observation', 'app-spec'].includes(name))) throw new Error('Unknown contract selection');
for (const [name, schema] of [['legacy-command', hostingCommands], ['observation', observationSchema], ['app-spec', appSpecSchema]]) {
  if (only.length && !only.includes(name)) continue;
  writeFileSync(new URL(`../src/${name}.schema.json`, import.meta.url), `${JSON.stringify(z.toJSONSchema(schema, { io: 'input' }), null, 2)}\n`);
}
