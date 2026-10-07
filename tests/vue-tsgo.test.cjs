// Which compiler npm/vue-tsgo.cjs runs, with stub packages: node --test tests/vue-tsgo.test.cjs
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const platform = `@chatium/tsc-rs-${process.platform}-${process.arch}`;

// A project with vue-tsgo and the given packages; each stub compiler prints its name.
function project(packages) {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), 'vue-tsgo-test-'));
	const write = (file, text, mode) => {
		fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
		fs.writeFileSync(path.join(root, file), text, { mode });
	};
	write('node_modules/@chatium/vue-ts-mapper-rs/package.json', '{}');
	fs.copyFileSync(path.join(__dirname, '../npm/vue-tsgo.cjs'), path.join(root, 'node_modules/@chatium/vue-ts-mapper-rs/vue-tsgo.cjs'));
	write('tsconfig.json', '{}');
	for (const [dir, name] of Object.entries(packages)) {
		write(`${dir}/package.json`, JSON.stringify({ name, version: name === 'typescript' ? '7.1.0' : '0.1.1' }));
		if (name === 'typescript') write(`${dir}/bin/tsc`, `console.log('typescript')`);
		if (name === platform) write(`${dir}/lib/tsc`, '#!/bin/sh\necho tsc-rs\n', 0o755);
	}
	return root;
}

function run(root, env = {}) {
	const result = spawnSync(process.execPath, ['node_modules/@chatium/vue-ts-mapper-rs/vue-tsgo.cjs'], {
		cwd: root,
		encoding: 'utf8',
		env: { ...process.env, VUE_TSGO_TSC: '', ...env },
	});
	fs.rmSync(root, { recursive: true, force: true });
	return (result.stdout + result.stderr).trim();
}

const typescript = { 'node_modules/typescript': 'typescript' };
const tscRs = { 'node_modules/@chatium/tsc-rs': '@chatium/tsc-rs', [`node_modules/${platform}`]: platform };

test('tsc-rs when it has a build for this machine', () => {
	assert.equal(run(project({ ...typescript, ...tscRs })), 'tsc-rs');
});

test('VUE_TSGO_TSC=typescript skips tsc-rs', () => {
	assert.equal(run(project({ ...typescript, ...tscRs }), { VUE_TSGO_TSC: 'typescript' }), 'typescript');
});

test('typescript when tsc-rs has no build for this machine', () => {
	assert.equal(run(project({ ...typescript, 'node_modules/@chatium/tsc-rs': '@chatium/tsc-rs' })), 'typescript');
});

test('the platform package next to the real path of tsc-rs (pnpm)', () => {
	const store = 'node_modules/.pnpm/@chatium+tsc-rs@0.1.1/node_modules';
	const root = project({ ...typescript, [`${store}/@chatium/tsc-rs`]: '@chatium/tsc-rs', [`${store}/${platform}`]: platform });
	fs.symlinkSync(path.join(root, store, '@chatium/tsc-rs'), path.join(root, 'node_modules/@chatium/tsc-rs'));
	assert.equal(run(root), 'tsc-rs');
});

test('an error without TypeScript 7', () => {
	assert.match(run(project({})), /TypeScript 7\.1 or later is required/);
});
