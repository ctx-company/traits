import { createHash } from 'node:crypto';
import { createWriteStream } from 'node:fs';
import { chmod, mkdtemp, mkdir, rename, rm } from 'node:fs/promises';
import { get as httpsGet } from 'node:https';
import { get as httpGet } from 'node:http';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pipeline } from 'node:stream/promises';
import * as tar from 'tar';

const RELEASE_BASE = 'https://github.com/ctx-company/traits/releases/download';
const SUPPORT = 'Install via curl: https://github.com/ctx-company/traits#installation; Homebrew: brew install ctx-company/tap/ctx.';

const TARGETS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

export function targetFor(platform, arch) {
  const target = TARGETS[`${platform}-${arch}`];
  if (!target) {
    throw new Error(`Unsupported platform ${platform}/${arch}. ${SUPPORT}`);
  }
  return target;
}

export function assetName(version, target) {
  return `ctx-v${version}-${target}.tar.gz`;
}

export function releaseUrls(version, target, base = RELEASE_BASE) {
  const archive = assetName(version, target);
  const prefix = `${base}/v${version}`;
  return { archive: `${prefix}/${archive}`, checksum: `${prefix}/${archive}.sha256` };
}

export function checksumFrom(text, archive) {
  const line = text.trim().split(/\r?\n/).find((entry) => entry.trim().endsWith(` ${archive}`));
  const match = line?.trim().match(/^([a-fA-F0-9]{64})\s+\*?\S+$/);
  if (!match) throw new Error(`Invalid checksum file for ${archive}`);
  return match[1].toLowerCase();
}

function request(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    const get = url.startsWith('http:') ? httpGet : httpsGet;
    get(url, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        if (redirects === 5) {
          reject(new Error(`Too many redirects for ${url}`));
          return;
        }
        resolve(request(new URL(response.headers.location, url).href, redirects + 1));
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`Download failed (${response.statusCode}) for ${url}`));
        return;
      }
      resolve(response);
    }).on('error', reject);
  });
}

async function download(url, destination) {
  await pipeline(await request(url), createWriteStream(destination));
}

async function text(url) {
  let result = '';
  for await (const chunk of await request(url)) result += chunk;
  return result;
}

async function sha256(file) {
  const hash = createHash('sha256');
  for await (const chunk of await (await import('node:fs')).createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

export async function install({ version, platform = process.platform, arch = process.arch, packageDir = dirname(fileURLToPath(import.meta.url)), releaseBase = RELEASE_BASE } = {}) {
  if (!version) throw new Error('Package version is required');
  const target = targetFor(platform, arch);
  const archive = assetName(version, target);
  const urls = releaseUrls(version, target, releaseBase);
  const binaryDir = join(packageDir, '..', 'binary');
  const binary = join(binaryDir, 'ctx');
  const temporary = await mkdtemp(join(tmpdir(), 'ctx-install-'));

  try {
    const downloaded = join(temporary, archive);
    await download(urls.archive, downloaded);
    const expected = checksumFrom(await text(urls.checksum), archive);
    if (await sha256(downloaded) !== expected) throw new Error(`Checksum verification failed for ${archive}`);

    const extract = join(temporary, 'extract');
    await mkdir(extract);
    await tar.x({ file: downloaded, cwd: extract, strict: true, filter: (path) => path === 'ctx' });
    const extracted = join(extract, 'ctx');
    await chmod(extracted, 0o755);
    await mkdir(binaryDir, { recursive: true });
    await rename(extracted, binary);
  } catch (error) {
    throw error;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
  const packageJson = JSON.parse(await (await import('node:fs/promises')).readFile(join(dirname(fileURLToPath(import.meta.url)), '..', 'package.json')));
  install({ version: packageJson.version }).catch((error) => {
    console.error(`ctx installation failed: ${error.message}\n${SUPPORT}`);
    process.exitCode = 1;
  });
}
