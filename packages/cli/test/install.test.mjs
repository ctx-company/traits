import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { chmod, copyFile, mkdtemp, mkdir, readFile, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import * as tar from 'tar';
import test from 'node:test';
import { assetName, checksumFrom, install, targetFor } from '../scripts/install.mjs';

test('maps release targets and rejects unsupported platforms', () => {
  assert.equal(targetFor('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(targetFor('darwin', 'x64'), 'x86_64-apple-darwin');
  assert.equal(targetFor('linux', 'arm64'), 'aarch64-unknown-linux-gnu');
  assert.equal(targetFor('linux', 'x64'), 'x86_64-unknown-linux-gnu');
  assert.throws(() => targetFor('win32', 'x64'), /curl.*Homebrew/);
});

test('requires a matching checksum filename', () => {
  assert.throws(() => checksumFrom('f'.repeat(64) + '  other.tar.gz\n', 'ctx.tar.gz'), /Invalid checksum/);
});

async function fixture({ checksum = false, symlinkEntry = false } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'ctx-test-'));
  const stage = join(root, 'stage');
  await (await import('node:fs/promises')).mkdir(stage);
  if (symlinkEntry) {
    await writeFile(join(stage, 'target'), '#!/bin/sh\nprintf fixture\n');
    await symlink('target', join(stage, 'ctx'));
  } else {
    await writeFile(join(stage, 'ctx'), '#!/bin/sh\nprintf fixture\n');
    await chmod(join(stage, 'ctx'), 0o755);
  }
  const name = assetName('1.2.3', 'x86_64-unknown-linux-gnu');
  const archive = join(root, name);
  await tar.c({ gzip: true, file: archive, cwd: stage }, ['ctx']);
  const digest = createHash('sha256').update(await readFile(archive)).digest('hex');
  const server = createServer(async (request, response) => {
    try {
      if (request.url?.endsWith('.sha256')) response.end(`${checksum ? '0'.repeat(64) : digest}  ${name}\n`);
      else response.end(await readFile(archive));
    } catch (error) {
      response.destroy(error);
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return { root, server, base: `http://127.0.0.1:${port}`, close: async () => { await new Promise((resolve) => server.close(resolve)); await rm(root, { recursive: true, force: true }); } };
}

test('installs only a checksum-verified ctx binary', async () => {
  const data = await fixture();
  const packageDir = join(data.root, 'package', 'scripts');
  await (await import('node:fs/promises')).mkdir(packageDir, { recursive: true });
  try {
    await install({ version: '1.2.3', platform: 'linux', arch: 'x64', packageDir, releaseBase: data.base });
    assert.equal(spawnSync(join(data.root, 'package', 'binary', 'ctx')).stdout.toString(), 'fixture');
  } finally { await data.close(); }
});

test('rejects an archive with a mismatched checksum', async () => {
  const data = await fixture({ checksum: true });
  const packageDir = join(data.root, 'package', 'scripts');
  await (await import('node:fs/promises')).mkdir(packageDir, { recursive: true });
  try {
    await assert.rejects(install({ version: '1.2.3', platform: 'linux', arch: 'x64', packageDir, releaseBase: data.base }), /Checksum verification failed/);
  } finally { await data.close(); }
});

test('rejects a checksum-valid symbolic-link ctx entry without installing a binary', async () => {
  const data = await fixture({ symlinkEntry: true });
  const packageDir = join(data.root, 'package', 'scripts');
  const binary = join(data.root, 'package', 'binary', 'ctx');
  await mkdir(packageDir, { recursive: true });
  try {
    await assert.rejects(install({ version: '1.2.3', platform: 'linux', arch: 'x64', packageDir, releaseBase: data.base }), /regular file/);
    await assert.rejects(stat(binary), { code: 'ENOENT' });
  } finally { await data.close(); }
});

test('shim gives recovery instructions when binary is absent', async () => {
  const root = await mkdtemp(join(tmpdir(), 'ctx-shim-test-'));
  await mkdir(join(root, 'bin'));
  await copyFile(fileURLToPath(new URL('../bin/ctx.mjs', import.meta.url)), join(root, 'bin', 'ctx.mjs'));
  try {
    const result = spawnSync(process.execPath, [join(root, 'bin', 'ctx.mjs')], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /--ignore-scripts.*curl.*Homebrew/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

async function waitForFile(path) {
  for (let attempts = 0; attempts < 50; attempts += 1) {
    try {
      await stat(path);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
  throw new Error(`Timed out waiting for ${path}`);
}

test('shim forwards SIGTERM to its running binary', async () => {
  const root = await mkdtemp(join(tmpdir(), 'ctx-shim-signal-test-'));
  const pidFile = join(root, 'child.pid');
  await mkdir(join(root, 'bin'));
  await mkdir(join(root, 'binary'));
  await copyFile(fileURLToPath(new URL('../bin/ctx.mjs', import.meta.url)), join(root, 'bin', 'ctx.mjs'));
  await writeFile(join(root, 'binary', 'ctx'), `#!/usr/bin/env node\nrequire('node:fs').writeFileSync(${JSON.stringify(pidFile)}, process.pid.toString());\nsetInterval(() => {}, 1000);\n`);
  await chmod(join(root, 'binary', 'ctx'), 0o755);
  const shim = spawn(process.execPath, [join(root, 'bin', 'ctx.mjs')]);
  try {
    await waitForFile(pidFile);
    const childPid = Number(await readFile(pidFile, 'utf8'));
    shim.kill('SIGTERM');
    const [, signal] = await once(shim, 'exit');
    assert.equal(signal, 'SIGTERM');
    assert.throws(() => process.kill(childPid, 0), { code: 'ESRCH' });
  } finally {
    if (!shim.killed) shim.kill('SIGKILL');
    await rm(root, { recursive: true, force: true });
  }
});
