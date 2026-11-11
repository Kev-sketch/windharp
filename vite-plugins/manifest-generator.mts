import { readFileSync, mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { basename, dirname, extname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { globSync } from 'glob';
import {
  Project,
  ts,
  type Node,
  type ObjectLiteralExpression,
  type ObjectLiteralElementLike,
} from 'ts-morph';

import type { PluginOption } from 'vite';

import { defaultConfig } from '../src/config/defaults';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Own Project instance — deliberately not shared with plugin-loader.mts.
const manifestProject = new Project({
  tsConfigFilePath: resolve(__dirname, '..', 'tsconfig.json'),
  skipAddingFilesFromTsConfig: true,
  skipLoadingLibFiles: true,
  skipFileDependencyResolution: true,
});

// Helper to extract a property's name from its node. Unlike the loader's
// OrThrow variant, quoted keys (StringLiteral-names) must not throw.
const getPropertyName = (prop: Node): string | null => {
  const kind = prop.getKind();
  if (
    kind === ts.SyntaxKind.PropertyAssignment ||
    kind === ts.SyntaxKind.ShorthandPropertyAssignment ||
    kind === ts.SyntaxKind.MethodDeclaration
  ) {
    const nameNode =
      prop.getFirstChildByKind(ts.SyntaxKind.Identifier) ??
      prop.getFirstChildByKind(ts.SyntaxKind.StringLiteral);
    if (nameNode) {
      const text = nameNode.getText();
      return nameNode.getKind() === ts.SyntaxKind.StringLiteral
        ? decodeStringLiteral(text)
        : text;
    }
  }
  return null;
};

// Discover plugin entry files exactly like plugin-importer.mts:
// both src/plugins/<id>/index.<ext> and single-file src/plugins/<id>.<ext>,
// excluding the utils pseudo-plugin.
const discoverPluginFiles = (): { id: string; file: string }[] => {
  const paths = globSync([
    'src/plugins/*/index.{js,ts,jsx,tsx}',
    'src/plugins/*.{js,ts,jsx,tsx}',
    '!src/plugins/utils/**/*',
    '!src/plugins/utils/*',
  ]);

  return paths.map((path) => {
    const base = basename(path);
    const dir = basename(dirname(path));
    const id =
      base === 'index.js' ||
      base === 'index.ts' ||
      base === 'index.jsx' ||
      base === 'index.tsx'
        ? dir
        : base.replace(extname(base), '');
    return { id, file: resolve(__dirname, '..', path) };
  });
};

// Recursively convert an ObjectLiteralExpression to plain JSON.
// Only statically safe kinds are allowed; anything else makes the key
// non-serializable and the caller skips the whole key.
const extractStaticValue = (
  node: Node,
): { ok: true; value: unknown } | { ok: false } => {
  const kind = node.getKind();
  if (kind === ts.SyntaxKind.StringLiteral) {
    return { ok: true, value: (node as ts.StringLiteralLike).getText().slice(1, -1) };
  }
  if (kind === ts.SyntaxKind.NumericLiteral) {
    return { ok: true, value: Number(node.getText()) };
  }
  if (
    kind === ts.SyntaxKind.TrueKeyword ||
    kind === ts.SyntaxKind.FalseKeyword
  ) {
    return { ok: true, value: node.getText() === 'true' };
  }
  if (kind === ts.SyntaxKind.NullKeyword) {
    return { ok: true, value: null };
  }
  if (kind === ts.SyntaxKind.ArrayLiteralExpression) {
    const arr: unknown[] = [];
    for (const el of (node as ObjectLiteralExpression & {
      getElements: () => Node[];
    }).getElements()) {
      const res = extractStaticValue(el);
      if (!res.ok) return { ok: false };
      arr.push(res.value);
    }
    return { ok: true, value: arr };
  }
  if (kind === ts.SyntaxKind.ObjectLiteralExpression) {
    const obj: Record<string, unknown> = {};
    for (const prop of (node as ObjectLiteralExpression).getProperties()) {
      const name = getPropertyName(prop);
      if (!name) continue; // spread / method etc. → key simply not emitted
      const propKind = prop.getKind();
      if (propKind === ts.SyntaxKind.ShorthandPropertyAssignment) {
        // shorthand value is an Identifier → non-static
        return { ok: false };
      }
      if (propKind === ts.SyntaxKind.PropertyAssignment) {
        const init = (prop as unknown as {
          getInitializer: () => Node | undefined;
        }).getInitializer();
        if (!init) return { ok: false };
        const res = extractStaticValue(init);
        if (!res.ok) return { ok: false };
        obj[name] = res.value;
      } else {
        // MethodDeclaration / other property kinds → non-static
        return { ok: false };
      }
    }
    return { ok: true, value: obj };
  }
  return { ok: false };
};

// Direct character-level unescape of a JS string literal body. Used only when
// the naive slice produces escapes that raw source bytes wouldn't (e.g. \u).
const decodeStringLiteral = (text: string): string => {
  const body = text.slice(1, -1);
  try {
    // JSON.parse accepts \uXXXX, \n, \t etc. and rejects real newlines, which
    // cannot occur inside a single string literal token anyway.
    return JSON.parse(`"${body.replace(/\n/g, '\\n')}"`);
  } catch {
    return body;
  }
};

// Clean string-literal extraction: prefer the AST compiler-escaped literal.
const extractStringLiteral = (node: Node): string | null => {
  const first = node.getFirstChildByKind(ts.SyntaxKind.StringLiteral);
  if (first) return decodeStringLiteral(first.getText());
  return null;
};

export function manifestGenerator(): PluginOption {
  const generate = (): {
    manifest: string;
    pluginCount: number;
    warnings: string[];
    fallbackIds: string[];
  } => {
    const warnings: string[] = [];
    const fallbackIds: string[] = [];

    const entries = discoverPluginFiles().map(({ id, file }) => {
      const fileContent = readFileSync(file, 'utf8');
      const src = manifestProject.createSourceFile(
        '_mg_' + basename(file),
        fileContent,
        { overwrite: true },
      );

      let objExpr: ObjectLiteralExpression | undefined;

      const defaultExportAssignment = src.getExportAssignment(
        (ea) => !ea.isExportEquals(),
      );
      if (defaultExportAssignment) {
        const expression = defaultExportAssignment.getExpression();
        if (expression.getKind() === ts.SyntaxKind.ObjectLiteralExpression) {
          objExpr = expression as ObjectLiteralExpression;
        } else if (expression.getKind() === ts.SyntaxKind.CallExpression) {
          const callExpr = expression as unknown as {
            getArguments: () => Node[];
            getExpression: () => Node;
          };
          if (
            callExpr.getArguments().length === 1 &&
            callExpr.getExpression().getText() === 'createPlugin'
          ) {
            const arg = callExpr.getArguments()[0];
            if (arg.getKind() === ts.SyntaxKind.ObjectLiteralExpression) {
              objExpr = arg as ObjectLiteralExpression;
            }
          }
        }
      }

      if (!objExpr) {
        const defaultExportDeclaration = src
          .getExportedDeclarations()
          .get('default');
        if (defaultExportDeclaration && defaultExportDeclaration.length > 0) {
          const expr = defaultExportDeclaration[0];
          if (expr.getKind() === ts.SyntaxKind.ObjectLiteralExpression) {
            objExpr = expr as ObjectLiteralExpression;
          } else if (expr.getKind() === ts.SyntaxKind.CallExpression) {
            const callExpr = expr as unknown as {
              getArguments: () => Node[];
              getExpression: () => Node;
            };
            if (
              callExpr.getArguments().length === 1 &&
              callExpr.getExpression().getText() === 'createPlugin'
            ) {
              const arg = callExpr.getArguments()[0];
              if (arg.getKind() === ts.SyntaxKind.ObjectLiteralExpression) {
                objExpr = arg as ObjectLiteralExpression;
              }
            }
          }
        }
      }

      // Plugin id: static string literal, else directory/file-name fallback.
      let pluginId: string | null = null;
      if (objExpr) {
        const idProp = objExpr
          .getProperties()
          .find(
            (p) =>
              getPropertyName(p) === 'id' &&
              p.getKind() === ts.SyntaxKind.PropertyAssignment,
          );
        if (idProp) {
          const init = (idProp as unknown as {
            getInitializer: () => Node | undefined;
          }).getInitializer();
          if (init && init.getKind() === ts.SyntaxKind.StringLiteral) {
            pluginId = decodeStringLiteral(init.getText());
          } else if (init) {
            warnings.push(`${id}: id non-static`);
          }
        }
      }
      if (pluginId === null) {
        pluginId = id;
        if (objExpr && !objExpr.getProperties().some((p) => getPropertyName(p) === 'id')) {
          // No id property at all → benign fallback, still recorded below.
        }
        fallbackIds.push(id);
      }

      // restartNeeded: BooleanLiteral → value; absent → false.
      let restartNeeded = false;
      let config: Record<string, unknown> = {};

      if (objExpr) {
        const propMap = new Map<string, ObjectLiteralElementLike>();
        for (const prop of objExpr.getProperties()) {
          const name = getPropertyName(prop);
          if (name) propMap.set(name, prop);
        }

        const restartProp = propMap.get('restartNeeded');
        if (restartProp && restartProp.getKind() === ts.SyntaxKind.PropertyAssignment) {
          const init = (restartProp as unknown as {
            getInitializer: () => Node | undefined;
          }).getInitializer();
          if (init) {
            if (init.getKind() === ts.SyntaxKind.TrueKeyword) restartNeeded = true;
            else if (init.getKind() === ts.SyntaxKind.FalseKeyword) restartNeeded = false;
            else warnings.push(`${pluginId}: restartNeeded non-static`);
          }
        }

        const configProp = propMap.get('config');
        if (
          configProp &&
          configProp.getKind() === ts.SyntaxKind.PropertyAssignment
        ) {
          const init = (configProp as unknown as {
            getInitializer: () => Node | undefined;
          }).getInitializer();
          const unwrapped = unwrapSatisfiesOrAs(init);
          if (
            unwrapped &&
            unwrapped.getKind() === ts.SyntaxKind.ObjectLiteralExpression
          ) {
            const obj = unwrapped as ObjectLiteralExpression;
            const res = extractConfigObject(obj, pluginId!, warnings);
            config = res;
          } else {
            warnings.push(`${pluginId}: config non-static`);
          }
        }
      }

      return { id: pluginId!, restartNeeded, config };
    });

    entries.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

    const manifest = JSON.stringify(
      { __defaults__: defaultConfig, plugins: entries },
      null,
      2,
    );

    return {
      manifest,
      pluginCount: entries.length,
      warnings,
      fallbackIds,
    };
  };

  const writeManifest = (): void => {
    const outDir = resolve(__dirname, '..', 'dist');
    const outPath = resolve(outDir, 'plugins-manifest.json');
    const { manifest, pluginCount, warnings, fallbackIds } = generate();
    mkdirSync(outDir, { recursive: true });
    writeFileSync(outPath, manifest + '\n');
    console.log(
      `[manifest-generator] wrote dist/plugins-manifest.json (${pluginCount} plugins, ${warnings.length} warnings)`,
    );
    if (fallbackIds.length > 0) {
      console.log(
        `[manifest-generator] id fallbacks used for: ${fallbackIds.join(', ')}`,
      );
    }
    for (const w of warnings) {
      console.warn(`[manifest-generator] warning: ${w}`);
    }
  };

  // Validate defaultConfig serializes cleanly at plugin load time.
  try {
    JSON.stringify(defaultConfig);
  } catch (e) {
    throw new Error(
      `defaultConfig from src/config/defaults.ts is not JSON-serializable: ${e}`,
    );
  }

  return {
    name: 'manifest-generator',
    writeBundle() {
      writeManifest();
    },
    configureServer() {
      writeManifest();
    },
  };
};

// `x satisfies T` / `x as T` wrappers around object literals are static-safe.
const unwrapSatisfiesOrAs = (node: Node | undefined): Node | undefined => {
  if (!node) return undefined;
  let cur = node;
  while (
    cur.getKind() === ts.SyntaxKind.SatisfiesExpression ||
    cur.getKind() === ts.SyntaxKind.AsExpression
  ) {
    cur = (cur as unknown as { getExpression: () => Node }).getExpression();
  }
  return cur;
};

// Object-level extraction: skips only the offending key, records a warning.
const extractConfigObject = (
  obj: ObjectLiteralExpression,
  pluginId: string,
  warnings: string[],
): Record<string, unknown> => {
  const out: Record<string, unknown> = {};
  for (const prop of obj.getProperties()) {
    const name = getPropertyName(prop);
    if (!name) continue;
    const kind = prop.getKind();
    if (kind === ts.SyntaxKind.ShorthandPropertyAssignment) {
      warnings.push(`${pluginId}: ${name} non-static`);
      continue;
    }
    if (kind !== ts.SyntaxKind.PropertyAssignment) {
      warnings.push(`${pluginId}: ${name} non-static`);
      continue;
    }
    const init = unwrapSatisfiesOrAs(
      (prop as unknown as { getInitializer: () => Node | undefined }).getInitializer(),
    );
    if (!init) {
      warnings.push(`${pluginId}: ${name} non-static`);
      continue;
    }
    const res = extractStaticValue(init);
    if (!res.ok) {
      warnings.push(`${pluginId}: ${name} non-static`);
      continue;
    }
    out[name] = res.value;
  }
  return out;
};

// String-literal value extraction (decodeStringLiteral does the unescape).
void extractStringLiteral;

// Intentionally no trailing exports: everything above is used.
