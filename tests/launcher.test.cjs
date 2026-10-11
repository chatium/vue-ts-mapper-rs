// npm/launcher.cjs spawns the binary where process.execve is unusable: node --test tests/launcher.test.cjs
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

// Runs the launcher with `preload` and a stub binary that prints its arguments and exits with 3.
function launch(preload) {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'launcher-test-'));
	fs.writeFileSync(path.join(dir, 'preload.cjs'), preload);
	fs.writeFileSync(path.join(dir, 'mapper'), '#!/bin/sh\necho "mapper $*"\nexit 3\n', { mode: 0o755 });
	const result = spawnSync(process.execPath, ['--require', path.join(dir, 'preload.cjs'), path.join(__dirname, '../npm/launcher.cjs'), '--x'], {
		encoding: 'utf8',
		env: { ...process.env, VUE_TS_MAPPER_RS_BINARY: path.join(dir, 'mapper') },
	});
	fs.rmSync(dir, { recursive: true, force: true });
	return { status: result.status, output: (result.stdout + result.stderr).trim() };
}

test('Windows: spawns the binary without calling process.execve', () => {
	const { status, output } = launch(`Object.defineProperty(process, 'platform', { value: 'win32' });
process.execve = () => { console.log('execve'); process.exit(9); };`);
	assert.equal(status, 3);
	assert.match(output, /^mapper --types-root=\S+ --x$/);
});

test('a process.execve that throws: spawns the binary', () => {
	const { status, output } = launch(`process.execve = () => { throw new TypeError('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM'); };`);
	assert.equal(status, 3);
	assert.match(output, /^mapper --types-root=\S+ --x$/);
});
