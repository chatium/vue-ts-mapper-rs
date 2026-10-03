// Runs the reference mapper over .vue files and writes one JSON line per file.
//
// usage: node dump.cjs <file-list | -> [options-json] > fixtures.jsonl
//   file-list: a text file with one .vue path per line ("-" reads stdin)
//   options-json: the mapper entry's `options` (default: {}); `typesRoot` is pinned so the
//   `/// <reference types>` line does not depend on where the reference is installed.
const fs = require('node:fs');
const { createContentMapper } = require('@vue/content-mapper/mapper.js');

const [listArg = '-', optionsArg = '{}'] = process.argv.slice(2);
const files = fs.readFileSync(listArg === '-' ? 0 : listArg, 'utf8').split('\n').filter(Boolean);
const options = { typesRoot: '/vue-ts-mapper-rs/types', target: 99, ...JSON.parse(optionsArg) };

const mapper = createContentMapper({ name: 'reference' });
const open = () => mapper.openProject({ configFileName: '', projectHandle: 'p', options, compilerOptions: {} });
open();

for (const [index, file] of files.entries()) {
	// the mapper and language-core keep every virtual code alive until the project closes
	if (index && index % 200 === 0) {
		mapper.closeProject('p');
		open();
	}
	const content = fs.readFileSync(file, 'utf8');
	let record;
	try {
		const r = mapper.transform({ fileName: file, content, projectHandle: 'p' });
		record = {
			file,
			text: r.text,
			extension: r.extension,
			mappings: r.mappings,
			directives: r.diagnosticDirectives?.directives ?? [],
		};
	}
	catch (error) {
		record = { file, error: String(error?.stack ?? error) };
	}
	process.stdout.write(JSON.stringify(record) + '\n');
}
