# vue-ts-mapper-rs

A [TypeScript 7 content mapper](https://github.com/microsoft/typescript-go) for Vue single-file
components, written in Rust. With it, `tsc` (TypeScript 7.1+, `tsgo`) type-checks `.vue` files
natively: the mapper turns each SFC into a virtual TypeScript file plus span mappings, and the
compiler reports diagnostics at the right places in the `.vue` source.

It is a port of the virtual code generation of [`@vue/language-core`](https://github.com/vuejs/language-tools)
(the v4 content-mapper branch, [vuejs/language-tools#6170](https://github.com/vuejs/language-tools/pull/6170)),
and produces the same output, byte for byte, as its Node.js mapper `@vue/content-mapper`, in a
fraction of the time and memory. It is generic: any Vue 3 project in TypeScript or JavaScript.

## Usage

Install TypeScript 7.1 and the mapper (prebuilt binaries for macOS, Linux and Windows, x64 and
arm64, are in the package):

```sh
npm i -D typescript@7.1.0-dev.20261003.1 \
  https://github.com/chatium/vue-ts-mapper-rs/releases/download/v0.1.1/chatium-vue-ts-mapper-rs-0.1.1.tgz
```

Register it in `tsconfig.json`:

```jsonc
{
  "compilerOptions": { "strict": true, "noEmit": true },
  "include": ["src/**/*.ts", "src/**/*.vue"],
  "contentMappers": [
    {
      "package": "@chatium/vue-ts-mapper-rs",
      "extensions": [".vue"],
      // only for CLI checks: skips the editor-feature flags of the mappings
      "options": { "languageFeatures": false }
    }
  ]
}
```

and run the compiler with external code enabled:

```sh
npx tsc --runExternalCode --noEmit
```

### Without touching the tsconfig

`vue-tsgo` runs the compiler with the mapper registered on top of an existing tsconfig (it
extends it from a temporary directory, since TypeScript resolves mapper packages from the
tsconfig's location). Nothing needs installing in the project:

```sh
npm exec --yes \
  --package=typescript@7.1.0-dev.20261003.1 \
  --package=https://github.com/chatium/vue-ts-mapper-rs/releases/download/v0.1.1/chatium-vue-ts-mapper-rs-0.1.1.tgz \
  -- vue-tsgo --noEmit
```

`vue-tsgo` takes the options of `tsc` (`-p` picks the tsconfig, `./tsconfig.json` by default).

### Options

The mapper entry's `options` take the Vue compiler options of `@vue/language-core` (`target`,
`lib`, `strictCssModules`, `fallthroughAttributes`, `dataAttributes`, `macros`, …), flattened
instead of nested in `vueCompilerOptions`. `vueCompilerOptions` of the tsconfig `extends` chain
are read too (under the entry's options), as are per-file `<!-- @key value -->` comments.
`target` defaults to the version of `node_modules/vue` and falls back to the latest.

Unsupported: Vue language plugins (`plugins`, e.g. Pug or Markdown templates) are JavaScript and
are not loaded; a `<template lang="pug">` is left unchecked, as without the plugin.

## Conformance

The output (generated text, span mappings and diagnostic directives) is compared with the
reference mapper's, file by file:

| corpus | SFCs | identical |
| --- | ---: | ---: |
| language-tools `test-workspace` | 381 | 378 |
| production SFCs (Chatium workspaces) | 10 178 | 10 177 |

The four differences are files with syntax errors, where swc's error recovery and TypeScript's
part ways. Type-checking 9 production workspaces with each mapper gives the same diagnostics at
the same positions.

The reference is `@vue/language-core` from vuejs/language-tools#6170 at `e75a638e`, with three
codegen fixes in [`tools/reference/language-tools.patch`](tools/reference/language-tools.patch):
a statement terminator after script blocks that do not end with a newline (otherwise the next
generated statement runs into the user's last one), `// @ts-ignore` on every line of a
component's props, and no non-null assertions in hoisted variables of JS scripts.

## Performance

`tsc --runExternalCode --noEmit` with TypeScript 7.1.0-dev.20261003.1 on an Apple M-series
laptop, the Node mapper against this one:

| SFCs | `.ts` files | `@vue/content-mapper` | vue-ts-mapper-rs |
| ---: | ---: | --- | --- |
| 3 571 | 4 694 | 24.7 s, 8.9 GB | 10.1 s, 6.4 GB |
| 2 901 | 3 283 | 11.8 s, 7.5 GB | 5.6 s, 4.0 GB |
| 2 725 | 3 966 | 17.4 s, 10.1 GB | 8.1 s, 5.5 GB |
| 1 931 | 4 790 | 21.8 s, 8.9 GB | 12.2 s, 6.9 GB |
| 903 | 1 813 | 5.9 s, 3.9 GB | 3.7 s, 2.3 GB |
| 810 | 2 003 | 6.3 s, 4.1 GB | 3.4 s, 2.5 GB |
| 680 | 1 211 | 3.6 s, 2.4 GB | 1.7 s, 1.2 GB |
| 513 | 1 190 | 4.7 s, 2.9 GB | 2.3 s, 1.4 GB |

(peak memory of the whole process tree). The mapper process itself peaks at 100–300 MB, where
the Node one takes 1–5 GB; the rest is the compiler. On its own, the mapper transforms about
1 000 SFCs per second on one core (≈1 ms per SFC, 13 KB on average) and scales across cores.

## How it works

- **SFC and template parsing**: [chatium/vue-sfc-swc](https://github.com/chatium/vue-sfc-swc),
  a byte-identical Rust port of `@vue/compiler-core` / `@vue/compiler-sfc` 3.5.38.
- **Script analysis**: [swc](https://swc.rs) parses `<script>` blocks and template expressions;
  the walkers reproduce what language-core reads from TypeScript's AST (token positions, leading
  comments, binding names and their scopes).
- **Codegen**: a transliteration of language-core's generators (`codegen/{script,template,style}`)
  writing into a segment buffer, with the same feature resolution, combine tokens and comment
  directives (`@vue-ignore`, `@vue-expect-error`, `@vue-skip`, `@vue-generic`).
- **Mappings**: Volar mappings, then the content mapper's span mappings and diagnostic
  directives, all in UTF-16 offsets as the protocol requires.
- **Protocol**: `Content-Length`-framed JSON-RPC on stdio (`initialize`, `openProject`,
  `closeProject`, `transform`); transforms run on a thread pool.

## Development

```sh
cargo test --release                     # unit tests + the language-tools fixtures
cargo run --release --example bench -- files.txt 8
```

To compare with the reference on any set of `.vue` files, build it once
(`tools/reference/build.sh`, needs Node.js and network), dump its output and diff:

```sh
node tools/reference/dump.cjs files.txt > ref.jsonl
cargo run --release --example conformance -- ref.jsonl --out /tmp/diffs
```

## License

MIT. Ported from vuejs/language-tools (MIT, © Johnson Chu); see [LICENSE](LICENSE).
