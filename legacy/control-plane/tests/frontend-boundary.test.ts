import { test, expect } from "bun:test";
import ts from "typescript";
import { existsSync, readdirSync, readFileSync, realpathSync } from "node:fs";
import { resolve, join } from "node:path";
const root = resolve(import.meta.dir, "../../..");
const files = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? files(join(dir, entry.name))
      : /\.[cm]?[jt]sx?$/.test(entry.name)
        ? [join(dir, entry.name)]
        : [],
  );
for (const app of ["auth", "console"])
  test(
    app +
      " frontend entrypoints have no reachable backend or service credentials",
    () => {
      const base = join(root, "apps", app),
        config = ts.readConfigFile(
          join(base, "tsconfig.json"),
          ts.sys.readFile,
        );
      const { options } = ts.parseJsonConfigFileContent(
        config.config,
        ts.sys,
        base,
      );
      const queue = [
          ...files(join(base, "src/app")),
          ...["next.config.ts", "src/proxy.ts"]
            .map((path) => join(base, path))
            .filter((path) => existsSync(path)),
        ],
        seen = new Set<string>(),
        violations: string[] = [];
      while (queue.length) {
        const file = realpathSync(queue.pop()!);
        if (seen.has(file)) continue;
        seen.add(file);
        const source = readFileSync(file, "utf8"),
          ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
        const allowedAuthSecret = (name: string) =>
          app === "console" &&
          file === join(root, "apps/console/src/lib/browser-auth-proxy.ts") &&
          name === "CLERK_SECRET_KEY";
        const visit = (node: ts.Node) => {
          let specifier: string | undefined;
          if (
            ts.isImportDeclaration(node) &&
            !node.importClause?.isTypeOnly &&
            !(
              node.importClause?.namedBindings &&
              ts.isNamedImports(node.importClause.namedBindings) &&
              !node.importClause.name &&
              node.importClause.namedBindings.elements.every(
                (e) => e.isTypeOnly,
              )
            ) &&
            ts.isStringLiteral(node.moduleSpecifier)
          )
            specifier = node.moduleSpecifier.text;
          if (
            ts.isExportDeclaration(node) &&
            !node.isTypeOnly &&
            node.moduleSpecifier &&
            ts.isStringLiteral(node.moduleSpecifier)
          )
            specifier = node.moduleSpecifier.text;
          if (
            ts.isCallExpression(node) &&
            (node.expression.kind === ts.SyntaxKind.ImportKeyword ||
              (ts.isIdentifier(node.expression) &&
                node.expression.text === "require")) &&
            node.arguments[0] &&
            ts.isStringLiteral(node.arguments[0])
          )
            specifier = node.arguments[0].text;
          if (specifier) {
            if (
              /^(?:pg|drizzle-orm|payload|@payloadcms|@webdock\/database-runtime|@webdock\/api|@polar-sh\/sdk|nodemailer|iron-session)(?:$|\/)/.test(
                specifier,
              ) ||
              specifier === "better-auth" ||
              (specifier.startsWith("better-auth/") &&
                !/^better-auth\/(?:react$|client(?:\/|$)|types$)/.test(
                  specifier,
                )) ||
              (specifier.startsWith("@better-auth/") &&
                !specifier.endsWith("/client"))
            )
              violations.push(file.slice(root.length + 1) + " → " + specifier);
            const module = ts.resolveModuleName(
              specifier,
              file,
              options,
              ts.sys,
            ).resolvedModule;
            if (module && !/\.d\.[cm]?ts$/.test(module.resolvedFileName)) {
              const target = realpathSync(module.resolvedFileName);
              if (
                !target.includes("/node_modules/") &&
                (target.startsWith(root + "/apps/") ||
                  target.startsWith(root + "/packages/"))
              )
                queue.push(target);
            }
          }
          if (
            ts.isPropertyAccessExpression(node) &&
            node.expression.getText(ast) === "process.env" &&
            /(?:SECRET|DATABASE_URL|ACCESS_TOKEN|PRIVATE_KEY|SMTP_PASSWORD|ENCRYPTION_KEY)/.test(
              node.name.text,
            ) &&
            !allowedAuthSecret(node.name.text)
          )
            violations.push(
              file.slice(root.length + 1) + " → env " + node.name.text,
            );
          if (
            ts.isElementAccessExpression(node) &&
            node.expression.getText(ast) === "process.env" &&
            ts.isStringLiteral(node.argumentExpression) &&
            /(?:SECRET|DATABASE_URL|ACCESS_TOKEN|PRIVATE_KEY|SMTP_PASSWORD|ENCRYPTION_KEY)/.test(
              node.argumentExpression.text,
            ) &&
            !allowedAuthSecret(node.argumentExpression.text)
          )
            violations.push(
              file.slice(root.length + 1) +
                " → env " +
                node.argumentExpression.text,
            );
          ts.forEachChild(node, visit);
        };
        visit(ast);
      }
      expect([...new Set(violations)]).toEqual([]);
    },
  );

test("Clerk server middleware and its secret are unreachable from every Console client component", () => {
  const base = join(root, "apps/console");
  const config = ts.readConfigFile(
    join(base, "tsconfig.json"),
    ts.sys.readFile,
  );
  const { options } = ts.parseJsonConfigFileContent(
    config.config,
    ts.sys,
    base,
  );
  const queue = files(join(base, "src")).filter((file) =>
    /^\s*["']use client["']/.test(readFileSync(file, "utf8")),
  );
  const seen = new Set<string>();
  while (queue.length) {
    const file = realpathSync(queue.pop()!);
    if (seen.has(file)) continue;
    seen.add(file);
    expect(file.endsWith("/browser-auth-proxy.ts")).toBe(false);
    const source = readFileSync(file, "utf8");
    // Next turns server-action imports into RPC references; their implementation stays on the server.
    if (/^\s*["']use server["']/.test(source)) continue;
    expect(source).not.toContain("CLERK_SECRET_KEY");
    const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
    const visit = (node: ts.Node) => {
      const specifier =
        (ts.isImportDeclaration(node) && !node.importClause?.isTypeOnly) ||
        (ts.isExportDeclaration(node) && !node.isTypeOnly)
          ? (node as ts.ImportDeclaration | ts.ExportDeclaration)
              .moduleSpecifier
          : ts.isCallExpression(node) &&
              node.expression.kind === ts.SyntaxKind.ImportKeyword
            ? node.arguments[0]
            : undefined;
      if (specifier && ts.isStringLiteral(specifier)) {
        expect(specifier.text).not.toBe("@clerk/nextjs/server");
        const target = ts.resolveModuleName(
          specifier.text,
          file,
          options,
          ts.sys,
        ).resolvedModule?.resolvedFileName;
        if (
          target &&
          !/\.d\.[cm]?ts$/.test(target) &&
          !target.includes("/node_modules/")
        )
          queue.push(target);
      }
      ts.forEachChild(node, visit);
    };
    visit(ast);
  }
});
