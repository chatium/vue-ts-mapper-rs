#!/usr/bin/env node
'use strict';
// `tsc` (TypeScript 7) with this mapper registered for `.vue` files, for projects whose tsconfig
// does not register it. TypeScript resolves a mapper package from the tsconfig's location, so the
// project's tsconfig is extended from a temporary directory where this package resolves.
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

const tsc = findTypeScript();
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
	const result = spawnSync(process.execPath, [tsc, '--runExternalCode', '-p', sidecar, ...rest], { stdio: 'inherit' });
	if (result.error) {
		console.error(result.error);
	}
	status = result.status ?? 1;
}
finally {
	fs.rmSync(dir, { recursive: true, force: true });
}
process.exit(status);

function findTypeScript() {
	for (const start of [__dirname, path.dirname(process.argv[1]), process.cwd()]) {
		for (let dir = start; ; dir = path.dirname(dir)) {
			const pkg = path.join(dir, 'node_modules', 'typescript', 'package.json');
			if (fs.existsSync(pkg)) {
				const { version } = JSON.parse(fs.readFileSync(pkg, 'utf8'));
				if (Number(version.split('.')[0]) >= 7) {
					return path.join(path.dirname(pkg), 'bin', 'tsc');
				}
			}
			if (path.dirname(dir) === dir) {
				break;
			}
		}
	}
	console.error('vue-tsgo: TypeScript 7.1 or later is required (npm i -D typescript@next)');
	process.exit(1);
}
