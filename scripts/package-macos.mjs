import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'darwin') throw new Error('macOS is required to verify and package the .app bundle.');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const { version } = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));
if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Invalid release version.');
const bundle = path.join(root, 'src-tauri/target/aarch64-apple-darwin/release/bundle');
const app = path.join(bundle, 'macos/TermTranslate.app');
const binary = path.join(app, 'Contents/MacOS/termtranslate');
const run = (command, args) => execFileSync(command, args, { encoding: 'utf8', cwd: root }).trim();
if (!existsSync(binary)) throw new Error('The Apple Silicon .app has not been built.');
const architectures = run('/usr/bin/lipo', ['-archs', binary]);
if (architectures !== 'arm64') throw new Error(`Expected only arm64, received: ${architectures}`);
run('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
const bundledVersion = run('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleShortVersionString', path.join(app, 'Contents/Info.plist')]);
if (bundledVersion !== version) throw new Error(`App version ${bundledVersion} does not match ${version}.`);
const dmgFiles = readdirSync(path.join(bundle, 'dmg')).filter(name => name.endsWith('.dmg'));
if (dmgFiles.length !== 1) throw new Error(`Expected one DMG, received ${dmgFiles.length}. Use a clean build directory.`);
const sourceDmg = path.join(bundle, 'dmg', dmgFiles[0]);
run('/usr/bin/hdiutil', ['verify', sourceDmg]);

const output = path.join(root, 'release/macos-arm64');
mkdirSync(output, { recursive: true });
const dmgName = `TermTranslate_${version}_macOS-arm64.dmg`;
const zipName = `TermTranslate-${version}-macOS-arm64.zip`;
copyFileSync(sourceDmg, path.join(output, dmgName));
// ditto preserves executable permissions, resource forks and bundle structure.
run('/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, path.join(output, zipName)]);
copyFileSync(path.join(root, 'docs/MACOS.md'), path.join(output, 'Mac使用说明.md'));
const signingIdentity = process.env.APPLE_SIGNING_IDENTITY || '-';
const info = {
  version, target: 'aarch64-apple-darwin', architectures,
  minimumSystemVersion: '12.0', builtAt: new Date().toISOString(),
  sourceCommit: process.env.GITHUB_SHA || null,
  signing: signingIdentity === '-' ? 'ad-hoc' : 'developer-id',
  notarizationRequested: Boolean(process.env.APPLE_ID && process.env.APPLE_PASSWORD && process.env.APPLE_TEAM_ID),
  verified: ['arm64-only', 'bundle-version', 'code-signature', 'dmg-integrity'],
  nativeSmoke: 'See native-smoke-macos.json when included; interactive Mac validation is separate.',
};
writeFileSync(path.join(output, 'build-info.json'), `${JSON.stringify(info, null, 2)}\n`);
const files = [dmgName, zipName, 'Mac使用说明.md', 'build-info.json'];
const checksums = files.map(name => `${createHash('sha256').update(readFileSync(path.join(output, name))).digest('hex')}  ${name}`);
writeFileSync(path.join(output, 'SHA256SUMS.txt'), `${checksums.join('\n')}\n`);
console.log(`Apple Silicon packages: ${output}`);
