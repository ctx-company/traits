#!/usr/bin/env node
import { accessSync, constants } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const binary = fileURLToPath(new URL('../binary/ctx', import.meta.url));

try {
  accessSync(binary, constants.X_OK);
} catch {
  console.error(
    'ctx binary is missing. npm lifecycle scripts may have been disabled with --ignore-scripts, or installation failed. Reinstall without --ignore-scripts, or install via curl: https://github.com/ctx-company/traits#installation; Homebrew: brew install ctx-company/tap/ctx.',
  );
  process.exitCode = 1;
  process.exit();
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

child.on('error', (error) => {
  console.error(`ctx failed to start: ${error.message}`);
  process.exitCode = 1;
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exitCode = code ?? 1;
  }
});
