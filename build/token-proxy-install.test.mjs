import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync, readlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const installer = resolve('install.sh');
function fixture(t, { os = 'Linux', arch = 'aarch64', corrupt = false, missing = false, failDownload = false, invalidJSON = false, oldApp = false, failMove = false } = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'token-proxy-install-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const tools = join(dir, 'tools'), bin = join(dir, 'custom bin'), apps = join(dir, 'apps');
  const payload = join(dir, 'payload');
  for (const p of [tools, bin, apps, payload]) mkdirSync(p);
  writeFileSync(join(bin, 'magpie'), 'official installation');
  const cpu = ['arm64', 'aarch64'].includes(arch) ? 'arm64' : 'amd64';
  const file = `token-proxy-${os === 'Darwin' ? 'darwin' : 'linux'}-${cpu}.${os === 'Darwin' ? 'zip' : 'tar.gz'}`;
  const archive = join(dir, file);
  const plist = id => `<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${id}</string></dict></plist>`;
  if (os === 'Darwin') {
    const contents = join(payload, 'Token Proxy.app/Contents');
    mkdirSync(join(contents, 'MacOS'), { recursive: true });
    writeFileSync(join(contents, 'MacOS/magpie'), 'new application', { mode: 0o755 });
    writeFileSync(join(contents, 'Info.plist'), plist('com.mxyhi.token-proxy.magpie'));
    const zipped = spawnSync('ditto', ['-c', '-k', '--keepParent', join(payload, 'Token Proxy.app'), archive]);
    assert.equal(zipped.status, 0, zipped.stderr?.toString());
    mkdirSync(join(apps, 'Token Proxy.app/Contents/MacOS'), { recursive: true });
    writeFileSync(join(apps, 'Token Proxy.app/Contents/Info.plist'), plist(oldApp ? 'com.mxyhi.token-proxy' : 'com.mxyhi.token-proxy.magpie'));
    writeFileSync(join(apps, 'Token Proxy.app/Contents/MacOS/magpie'), 'previous application');
  } else {
    for (const name of ['token-proxy', 'LICENSE', 'THIRD_PARTY_NOTICES']) writeFileSync(join(payload, name), name === 'token-proxy' ? 'new application' : 'license');
    assert.equal(spawnSync('tar', ['-czf', archive, '-C', payload, 'token-proxy', 'LICENSE', 'THIRD_PARTY_NOTICES']).status, 0);
    writeFileSync(join(bin, 'token-proxy'), 'previous application');
  }
  const tag = 'prelease-0.2.0-4';
  const url = `https://github.com/mxyhi/token_proxy/releases/download/${tag}`;
  const release = { tag_name: tag, prerelease: true, draft: false, published_at: '2026-11-25T06:36:16Z', assets: [file, 'SHA256SUMS'].filter(n => !missing || n !== file).map(name => ({ name, size: 10, state: 'uploaded', browser_download_url: `${url}/${name}` })) };
  writeFileSync(join(dir, 'release.json'), JSON.stringify(release));
  const releases = [
    { ...release, tag_name: 'prelease-0.2.0-99', draft: true, published_at: '2026-12-01T00:00:00Z' },
    { ...release, tag_name: 'v0.1.231', prerelease: false },
    { ...release, tag_name: 'prelease-0.2.0-3', published_at: '2026-11-24T00:00:00Z' },
    release,
  ];
  writeFileSync(join(dir, 'releases.json'), invalidJSON ? '{bad' : JSON.stringify(releases));
  const sum = corrupt ? '0'.repeat(64) : createHash('sha256').update(readFileSync(archive)).digest('hex');
  writeFileSync(join(dir, 'SHA256SUMS'), `${sum}  ${file}\n`);
  writeFileSync(join(tools, 'uname'), `#!/bin/sh\ncase "$1" in -s) echo ${os};; -m) echo ${arch};; esac\n`, { mode: 0o755 });
  writeFileSync(join(tools, 'curl'), `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2), url = args.at(-1), out = args[args.indexOf('-o') + 1];
fs.appendFileSync(process.env.TEST_DIR + '/requests', url + '\\n');
let source;
if (url === 'https://api.github.com/repos/mxyhi/token_proxy/releases?per_page=100') source = 'releases.json';
else if (url === 'https://api.github.com/repos/mxyhi/token_proxy/releases/tags/${tag}') source = 'release.json';
else if (url === '${url}/${file}') { if (${failDownload}) process.exit(22); source = '${file}'; }
else if (url === '${url}/SHA256SUMS') source = 'SHA256SUMS';
else throw new Error('unexpected URL: ' + url);
fs.copyFileSync(path.join(process.env.TEST_DIR, source), out);
`, { mode: 0o755 });
  if (failMove) writeFileSync(join(tools, 'mv'), `#!/bin/sh\ncase "$1" in */new/Token\\ Proxy.app) exit 1;; esac\nexec /bin/mv "$@"\n`, { mode: 0o755 });
  const run = (args = [], extra = {}) => spawnSync('sh', [installer, ...args], {
    env: { PATH: `${tools}:${process.env.PATH}`, TEST_DIR: dir, TOKEN_PROXY_BIN_DIR: bin, TOKEN_PROXY_APP_DIR: apps, ...extra },
    encoding: 'utf8', timeout: 15000,
  });
  return { run, dir, bin, apps, file };
}

for (const arch of ['aarch64', 'x86_64']) test(`Linux ${arch}: selects published prerelease, checks archive and keeps official command`, t => {
  const f = fixture(t, { arch });
  const r = f.run();
  assert.equal(r.status, 0, r.stderr);
  assert.equal(readFileSync(join(f.bin, 'token-proxy'), 'utf8'), 'new application');
  assert.equal(readFileSync(join(f.bin, 'magpie'), 'utf8'), 'official installation');
  assert.match(r.stdout, /GTK 3/);
});
for (const [name, options] of [
  ['checksum mismatch', { corrupt: true }], ['missing asset', { missing: true }],
  ['network failure', { failDownload: true }], ['malformed response', { invalidJSON: true }],
]) test(`${name}: fails and keeps previous installation`, t => {
  const f = fixture(t, options), r = f.run();
  assert.notEqual(r.status, 0, r.stdout);
  assert.equal(readFileSync(join(f.bin, 'token-proxy'), 'utf8'), 'previous application');
});
test('pinned prerelease fetches its metadata and does not query latest', t => {
  const f = fixture(t), r = f.run([], { TOKEN_PROXY_TAG: 'prelease-0.2.0-4' });
  assert.equal(r.status, 0, r.stderr);
  assert.doesNotMatch(readFileSync(join(f.dir, 'requests'), 'utf8'), /per_page/);
});
test('invalid tags and unknown flags make no requests', t => {
  const f = fixture(t);
  assert.notEqual(f.run([], { TOKEN_PROXY_TAG: '../latest' }).status, 0);
  assert.notEqual(f.run(['--uninstall']).status, 0);
  assert.equal(existsSync(join(f.dir, 'requests')), false);
});
for (const [name, options, succeeds] of [
  ['upgrade', {}, true], ['old Tauri app', { oldApp: true }, false], ['failed replacement restores backup', { failMove: true }, false],
]) test(`macOS ${name}`, { skip: process.platform !== 'darwin' }, t => {
  const f = fixture(t, { os: 'Darwin', ...options }), r = f.run();
  assert.equal(r.status === 0, succeeds, r.stderr);
  assert.equal(readFileSync(join(f.apps, 'Token Proxy.app/Contents/MacOS/magpie'), 'utf8'), succeeds ? 'new application' : 'previous application');
  assert.equal(readFileSync(join(f.bin, 'magpie'), 'utf8'), 'official installation');
  if (succeeds) assert.equal(readlinkSync(join(f.bin, 'token-proxy')), join(f.apps, 'Token Proxy.app/Contents/MacOS/magpie'));
});
