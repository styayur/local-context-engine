import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, unlinkSync, rmdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const require = createRequire(import.meta.url);
const api = require('typescript');
const nativePackage = require.resolve('@typescript/native/package.json');
const native = JSON.parse(readFileSync(nativePackage, 'utf8'));
assert.match(api.version, /^6\.0\./, 'ESLint must load the supported TypeScript 6.0 API');
assert.match(native.version, /^7\.0\./, 'typecheck/build must use the requested TypeScript 7.0 CLI');
assert.equal(typeof api.createSourceFile, 'function');
const parsed = api.createSourceFile('lint-fixture.ts', 'const value: number = 42;', api.ScriptTarget.Latest, true);
assert.equal(parsed.parseDiagnostics.length, 0);
const compiler = resolve(dirname(nativePackage), native.bin.tsc);
const version = execFileSync(process.execPath, [compiler, '--version'], { encoding: 'utf8' }).trim();
assert.equal(version, `Version ${native.version}`);
const folder = mkdtempSync(join(tmpdir(), 'lce-ts-cli-'));
const fixture = join(folder, 'invalid.ts');
try {
  writeFileSync(fixture, 'const mustBeNumber: number = "own invalid fixture";\n');
  const result = spawnSync(process.execPath, [compiler, '--ignoreConfig', '--strict', '--noEmit', '--skipLibCheck', '--target', 'es2022', fixture], { encoding: 'utf8' });
  assert.ok(result.status > 0, 'the real compiler must reject the invalid type assignment');
  assert.match(result.stdout + result.stderr, /TS2322/);
} finally {
  unlinkSync(fixture);
  rmdirSync(folder);
}
process.stdout.write(JSON.stringify({ compiler: version, eslintCompilerApi: api.version, invalidAssignmentRejected: true }) + '\n');
