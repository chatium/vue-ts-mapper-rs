'use strict';
// TypeScript starts content mappers in their package directory with the `exec` of package.json.
// This picks the native binary for the platform and hands it the process.
const fs = require('node:fs');
const path = require('node:path');

const exe = process.platform === 'win32' ? 'vue-ts-mapper-rs.exe' : 'vue-ts-mapper-rs';
const binary = process.env.VUE_TS_MAPPER_RS_BINARY
	?? path.join(__dirname, 'bin', `${process.platform}-${process.arch}`, exe);
if (!fs.existsSync(binary)) {
	console.error(`vue-ts-mapper-rs: no binary for ${process.platform}-${process.arch} (${binary})`);
	process.exit(1);
}
const args = [`--types-root=${path.join(__dirname, 'types')}`, ...process.argv.slice(2)];

if (typeof process.execve === 'function') {
	// Node >= 22.15: replace this process, so nothing sits between TypeScript and the mapper
	process.execve(binary, [binary, ...args], process.env);
}
else {
	const { status, error } = require('node:child_process').spawnSync(binary, args, { stdio: 'inherit' });
	if (error) {
		console.error(error);
	}
	process.exit(status ?? 1);
}
