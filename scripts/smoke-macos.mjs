import { spawn } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Native smoke checks require an Apple Silicon Mac.');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const data = path.join(root, '.artifacts/native-smoke-macos');
mkdirSync(data, { recursive: true });
const binary = path.join(root, 'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/TermTranslate.app/Contents/MacOS/termtranslate');
const child = spawn(binary, ['--smoke-test'], {
  cwd: root, env: { ...process.env, TERMTRANSLATE_DATA_DIR: data }, stdio: 'inherit', detached: true,
});
let timedOut = false;
const timer = setTimeout(() => {
  timedOut = true;
  try { process.kill(-child.pid, 'SIGKILL'); } catch { /* The process may have exited. */ }
}, 90_000);
try {
  const code = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', resolve);
  });
  if (timedOut) throw new Error('Native smoke checks timed out after 90 seconds.');
  const reportPath = path.join(data, 'native-smoke.json');
  const report = JSON.parse(readFileSync(reportPath, 'utf8'));
  const output = path.join(root, 'release/macos-arm64');
  mkdirSync(output, { recursive: true });
  copyFileSync(reportPath, path.join(output, 'native-smoke-macos.json'));
  console.log(JSON.stringify(report, null, 2));
  if (code !== 0 || report.passed !== true || report.platform !== 'macos') throw new Error('Native smoke checks failed; see native-smoke-macos.json.');
} finally {
  clearTimeout(timer);
}
