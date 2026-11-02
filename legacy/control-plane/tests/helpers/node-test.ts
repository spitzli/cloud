// node:test pieces Bun's runner lacks, in the shape the ported suites use.

// mockMethod replaces a property the way node:test does (an own property defined on the object, which
// also works on proxies such as lazyAuth); `.mock.restore()` or `restoreMocks` after each test undoes it.
const active = new Set<() => void>();

export function mockMethod<T extends object, K extends keyof T>(
  object: T,
  name: K,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  implementation: (...args: any[]) => unknown,
) {
  const own = Object.getOwnPropertyDescriptor(object, name);
  Object.defineProperty(object, name, {
    value: implementation,
    configurable: true,
    writable: true,
    enumerable: own?.enumerable ?? false,
  });
  const restore = () => {
    if (!active.delete(restore)) return;
    if (own) Object.defineProperty(object, name, own);
    else delete object[name];
  };
  active.add(restore);
  return { mock: { restore } };
}

export function restoreMocks() {
  for (const restore of [...active].reverse()) restore();
}

/** Runs a former subtest inline; Bun has no nested tests. A failure names the step it came from. */
export async function subtest(name: string, body: () => unknown) {
  try {
    await body();
  } catch (error) {
    if (error instanceof Error) error.message = `${name}: ${error.message}`;
    throw error;
  }
}
