import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { toNodeHandler } from 'srvx/node';
import configure from '../vite.config.ts';

test('development request clones retain JSON mutation methods, bodies and auth headers', async () => {
  configure({ command: 'serve', mode: 'development' });
  const server = createServer(toNodeHandler(async request => {
    const clone = new Request(request, { headers: new Headers(request.headers) });
    return Response.json({ method: clone.method, data: await clone.json(), cookie: clone.headers.get('cookie') });
  }));
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    for (const method of ['POST', 'PUT', 'PATCH']) {
      const response = await fetch(`http://127.0.0.1:${server.address().port}`, {
        method, headers: { 'content-type': 'application/json', cookie: 'fixture-session=owned' },
        body: JSON.stringify({ title: method }),
      });
      assert.equal(response.status, 200);
      assert.deepEqual(await response.json(), { method, data: { title: method }, cookie: 'fixture-session=owned' });
    }
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});
