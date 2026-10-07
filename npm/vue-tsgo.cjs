#!/usr/bin/env node
'use strict';
// `tsc` (TypeScript 7) with this mapper registered for `.vue` files, for projects whose tsconfig
// does not register it. TypeScript resolves a mapper package from the tsconfig's location, so the
// project's tsconfig is extended from a temporary directory where this package resolves.
// The compiler is @chatium/tsc-rs (the Rust port of TypeScript 7) when it is installed with a build
// for this machine, else `typescript` 7. VUE_TSGO_TSC=typescript skips tsc-rs.
//
//   npx vue-tsgo [-p tsconfig.json] [tsc options...]
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const args = process.argv.slice(2);
let project = 'tsconfig.json';
const rest = [];
for (let i = 0; i < args.length; i++) {
	if (args[i] === '-p' || args[i] === '--project') {
		project = args[++i];
	}
	else if (args[i].startsWith('--project=')) {
		project = args[i].slice('--project='.length);
	}
	else {
		rest.push(args[i]);
	}
}
let config = path.resolve(project);
if (fs.existsSync(config) && fs.statSync(config).isDirectory()) {
	config = path.join(config, 'tsconfig.json');
}

const tsc = findTsc();
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'vue-tsgo-'));
let status = 1;
try {
	fs.mkdirSync(path.join(dir, 'node_modules', '@chatium'), { recursive: true });
	fs.symlinkSync(__dirname, path.join(dir, 'node_modules', '@chatium', 'vue-ts-mapper-rs'), 'junction');
	const sidecar = path.join(dir, 'tsconfig.json');
	fs.writeFileSync(sidecar, JSON.stringify({
		extends: config,
		contentMappers: [{
			package: '@chatium/vue-ts-mapper-rs',
			extensions: ['.vue'],
			options: { languageFeatures: false },
		}],
	}));
	const result = spawnSync(tsc[0], [...tsc.slice(1), '--runExternalCode', '-p', sidecar, ...rest], { stdio: 'inherit' });
	if (result.error) {
		console.error(result.error);
	}
	if (result.signal) {
		// e.g. SIGKILL from the OOM killer: report it, and exit the way a shell does
		console.error(`vue-tsgo: tsc killed by ${result.signal}`);
		status = 128 + os.constants.signals[result.signal];
	}
	else {
		status = result.status ?? 1;
	}
}
finally {
	fs.rmSync(dir, { recursive: true, force: true });
}
process.exit(status);

// The command line of the compiler.
function findTsc() {
	const starts = [__dirname, path.dirname(process.argv[1]), process.cwd()];
	const tscRs = process.env.VUE_TSGO_TSC !== 'typescript' && findPackage(starts, '@chatium/tsc-rs');
	if (tscRs) {
		// Its native tsc is in the platform package next to it (pnpm keeps that next to the real path).
		const platform = findPackage([fs.realpathSync(tscRs)], `@chatium/tsc-rs-${process.platform}-${process.arch}`);
		const exe = platform && path.join(platform, 'lib', process.platform === 'win32' ? 'tsc.exe' : 'tsc');
		if (exe && fs.existsSync(exe)) {
			return [exe];
		}
	}
	const typescript = findPackage(starts, 'typescript', pkg => Number(readJson(path.join(pkg, 'package.json')).version.split('.')[0]) >= 7);
	if (typescript) {
		return [process.execPath, path.join(typescript, 'bin', 'tsc')];
	}
	console.error('vue-tsgo: TypeScript 7.1 or later is required (npm i -D typescript@next)');
	process.exit(1);
}

// The first node_modules/<name> up from each start that `accept`s.
function findPackage(starts, name, accept = () => true) {
	for (const start of starts) {
		for (let dir = start; ; dir = path.dirname(dir)) {
			const pkg = path.join(dir, 'node_modules', name);
			if (fs.existsSync(path.join(pkg, 'package.json')) && accept(pkg)) {
				return pkg;
			}
			if (path.dirname(dir) === dir) {
				break;
			}
		}
	}
}

function readJson(file) {
	return JSON.parse(fs.readFileSync(file, 'utf8'));
}
