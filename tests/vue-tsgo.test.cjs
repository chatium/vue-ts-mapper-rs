// Which compiler npm/vue-tsgo.cjs runs, with stub packages: node --test tests/vue-tsgo.test.cjs
'use strict';
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

// The platform packages vue-tsgo looks for on this machine (CI and macOS have glibc or no libc suffix).
const tsrsPlatform = process.platform === 'linux' ? `@ts-rs/linux-${process.arch}-gnu` : `@ts-rs/${process.platform}-${process.arch}`;
const tscRsPlatform = `@chatium/tsc-rs-${process.platform}-${process.arch}`;
const binaries = { [tsrsPlatform]: 'tsrs', [`@ts-rs/linux-${process.arch}-musl`]: 'tsrs', [tscRsPlatform]: 'lib/tsc' };

// A project with vue-tsgo and the given packages (dir: name); each stub compiler prints
// `<package>|<arguments>|<TSRS_RETIRE_MIN>`.
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
		write(`${dir}/package.json`, JSON.stringify({ name, version: name === 'typescript' ? '7.1.0' : '0.1.0' }));
		if (name === 'typescript') write(`${dir}/bin/tsc`, "console.log(['typescript', process.argv.slice(2).join(' '), process.env.TSRS_RETIRE_MIN ?? ''].join('|'))");
		if (binaries[name]) write(`${dir}/${binaries[name]}`, `#!/bin/sh\necho "${name}|$*|$TSRS_RETIRE_MIN"\n`, 0o755);
	}
	return root;
}

function run(root, env = {}) {
	const result = spawnSync(process.execPath, ['node_modules/@chatium/vue-ts-mapper-rs/vue-tsgo.cjs'], {
		cwd: root,
		encoding: 'utf8',
		env: { ...process.env, VUE_TSGO_TSC: '', VUE_TSGO_MAX_MEMORY: '', TSRS_RETIRE_MIN: '', ...env },
	});
	fs.rmSync(root, { recursive: true, force: true });
	return (result.stdout + result.stderr).trim();
}

// The compiler that ran, its arguments and TSRS_RETIRE_MIN.
function ran(root, env) {
	const [compiler, args, retireMin] = run(root, env).split('|');
	return { compiler, args, retireMin };
}

const typescript = { 'node_modules/typescript': 'typescript' };
const tscRs = { 'node_modules/@chatium/tsc-rs': '@chatium/tsc-rs', [`node_modules/${tscRsPlatform}`]: tscRsPlatform };
const tsrs = { 'node_modules/tsrs': 'tsrs', [`node_modules/${tsrsPlatform}`]: tsrsPlatform };

test('tsrs first', () => {
	assert.equal(ran(project({ ...typescript, ...tscRs, ...tsrs })).compiler, tsrsPlatform);
});

test('VUE_TSGO_TSC=tsc-rs skips tsrs', () => {
	assert.equal(ran(project({ ...typescript, ...tscRs, ...tsrs }), { VUE_TSGO_TSC: 'tsc-rs' }).compiler, tscRsPlatform);
});

test('VUE_TSGO_TSC=typescript skips the Rust ports', () => {
	assert.equal(ran(project({ ...typescript, ...tscRs, ...tsrs }), { VUE_TSGO_TSC: 'typescript' }).compiler, 'typescript');
});

test('tsc-rs when tsrs has no build for this machine', () => {
	assert.equal(ran(project({ ...typescript, ...tscRs, 'node_modules/tsrs': 'tsrs' })).compiler, tscRsPlatform);
});

test('typescript when neither port has a build for this machine', () => {
	assert.equal(ran(project({ ...typescript, 'node_modules/tsrs': 'tsrs', 'node_modules/@chatium/tsc-rs': '@chatium/tsc-rs' })).compiler, 'typescript');
});

test('Linux: the static musl build of tsrs when the GNU one is missing', { skip: process.platform !== 'linux' }, () => {
	const musl = `@ts-rs/linux-${process.arch}-musl`;
	assert.equal(ran(project({ ...typescript, 'node_modules/tsrs': 'tsrs', [`node_modules/${musl}`]: musl })).compiler, musl);
});

test('the platform package next to the real path of the port (pnpm)', () => {
	const store = 'node_modules/.pnpm/tsrs@0.1.0/node_modules';
	const root = project({ ...typescript, [`${store}/tsrs`]: 'tsrs', [`${store}/${tsrsPlatform}`]: tsrsPlatform });
	fs.symlinkSync(path.join(root, store, 'tsrs'), path.join(root, 'node_modules/tsrs'));
	assert.equal(ran(root).compiler, tsrsPlatform);
});

test('VUE_TSGO_MAX_MEMORY: tsrs --maxMemory, checkers retired down to 16 MB', () => {
	const { compiler, args, retireMin } = ran(project({ ...typescript, ...tsrs }), { VUE_TSGO_MAX_MEMORY: '1500M' });
	assert.equal(compiler, tsrsPlatform);
	assert.match(args, /^--maxMemory 1500M --runExternalCode /);
	assert.equal(retireMin, '16M');
});

test('VUE_TSGO_MAX_MEMORY keeps a TSRS_RETIRE_MIN that is set', () => {
	assert.equal(ran(project({ ...typescript, ...tsrs }), { VUE_TSGO_MAX_MEMORY: '1G', TSRS_RETIRE_MIN: '64M' }).retireMin, '64M');
});

test('VUE_TSGO_MAX_MEMORY: no --maxMemory for the other compilers', () => {
	assert.match(ran(project({ ...typescript, ...tscRs }), { VUE_TSGO_MAX_MEMORY: '1G' }).args, /^--runExternalCode /);
	assert.match(ran(project(typescript), { VUE_TSGO_MAX_MEMORY: '1G' }).args, /^--runExternalCode /);
});

test('an error without TypeScript 7', () => {
	assert.match(run(project({})), /TypeScript 7\.1 or later is required/);
});
