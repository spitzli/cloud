// Capture the retained first-party public schema; no runtime TypeScript dependency.
import { writeFileSync } from 'node:fs';
import { z } from 'zod';
import { customerInput, projectInput, listInput } from '../../../packages/core/src/registry/contracts.ts';
writeFileSync(new URL('../src/resource-registry.schema.json', import.meta.url), `${JSON.stringify({customer:z.toJSONSchema(customerInput),project:z.toJSONSchema(projectInput),filters:z.toJSONSchema(listInput)},null,2)}\n`);
