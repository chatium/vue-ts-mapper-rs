# vue-ts-mapper-rs

A [TypeScript 7 content mapper](https://github.com/microsoft/typescript-go) for Vue single-file
components, written in Rust. With it, `tsc` (TypeScript 7.1+, `tsgo`) type-checks `.vue` files
natively: the mapper turns each SFC into a virtual TypeScript file plus span mappings, and the
compiler reports diagnostics at the right places in the `.vue` source.

The virtual code generation is a port of [`@vue/language-core`](https://github.com/vuejs/language-tools)
(the v4 content-mapper branch, [vuejs/language-tools#6170](https://github.com/vuejs/language-tools/pull/6170)),
with the checking semantics of `vue-tsc` 3.3 on TypeScript 6 where the two differ (see
[Compared with vue-tsc](#compared-with-vue-tsc)). It is generic: any Vue 3 project in TypeScript
or JavaScript, and it checks a project in a fraction of `vue-tsc`'s time and memory.

## Usage

Install TypeScript 7.1 and the mapper (prebuilt binaries for macOS, Linux and Windows, x64 and
arm64, are in the package):

```sh
npm i -D typescript@7.1.0-dev.20261003.1 \
  https://github.com/chatium/vue-ts-mapper-rs/releases/download/v0.2.0/chatium-vue-ts-mapper-rs-0.2.0.tgz
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
  --package=https://github.com/chatium/vue-ts-mapper-rs/releases/download/v0.2.0/chatium-vue-ts-mapper-rs-0.2.0.tgz \
  -- vue-tsgo --noEmit
```

`vue-tsgo` takes the options of `tsc` (`-p` picks the tsconfig, `./tsconfig.json` by default).
A tsconfig that uses `${configDir}` or relative `types` entries resolves them against that
temporary directory; register the mapper in the tsconfig instead.

### Options

The mapper entry's `options` take the Vue compiler options of `@vue/language-core` (`target`,
`lib`, `strictTemplates`, `checkUnknownProps`, `strictCssModules`, `fallthroughAttributes`,
`dataAttributes`, `macros`, …), flattened instead of nested in `vueCompilerOptions`.
`vueCompilerOptions` of the tsconfig `extends` chain are read too (under the entry's options), as
are per-file `<!-- @key value -->` comments. `target` defaults to the version of
`node_modules/vue` and falls back to the latest.

As in `vue-tsc` 3.3, templates are lenient by default: fallthrough attributes, unknown events,
directives and components are allowed and `v-model` events are not checked.
`strictTemplates: true` turns on `checkUnknownProps`, `checkUnknownEvents`,
`checkUnknownDirectives`, `checkUnknownComponents` and `strictVModel` (each can be set on its own).

Unsupported: Vue language plugins (`plugins`, e.g. Pug or Markdown templates) are JavaScript and
are not loaded; a `<template lang="pug">` is left unchecked, as without the plugin.

## Compared with vue-tsc

Checked against `vue-tsc` 3.3.12 and 3.3.11 (TypeScript 6.0.3) by type-checking whole projects
and diffing the diagnostics by file, position and code: 12 production workspaces (≈11 000 SFCs)
and 8 open-source projects (vitesse, vue-pure-admin, vitepress and its docs, slidev,
element-plus, halo, gitea). On the open-source ones the output is identical except for the items
below; no `vue-tsc` diagnostic is hidden by the mapper's suppression of generated code (checked
by rerunning with `VUE_TS_MAPPER_KEEP_UNMAPPED=1`).

What differs, and why:

- **JS components are typed.** With `allowJs: false`, `vue-tsc` types an import of a `.vue` file
  with a JavaScript `<script>` as `any` and reports TS7016 at the import. TypeScript 7 types it,
  so props, events and slots of such components are checked (wrong prop types, `null` passed to
  a `Number` prop, …) and TS7016 and the follow-up implicit-`any` errors are gone.
- **TypeScript 7 itself**, in `.ts` and `.vue` alike: an overload error (TS2769) is reported at the
  argument instead of the call (so a `// @ts-ignore` two lines above no longer covers it),
  messages are more specific (TS2552 "Did you mean", TS2741 instead of TS2345), destructuring of
  a possibly-`undefined` value is reported (TS2488), the first excess property of an object
  literal follows source order, JS-style `Fn.prototype = { … }` in `.ts` files no longer types
  `this`, and `TuplifyUnion`-style recursive types can hit TS5114.
- **`vue-tsc` false positives not reproduced**: errors starting in generated code that `vue-tsc`
  drops (it misses e.g. a wrong argument count of an imported function in a template),
  `.value` of a function / class / enum / import used in a narrowing position (3.3.12),
  references inside `v-if="false"` blocks that lose the narrowing of enclosing `v-if`s,
  `v-for` variables lost after a slot (3.3.12), explicit `import { defineProps } from 'vue'`
  (TS2440, 3.3.11), and syntax errors in generated code that `vue-tsc` hides (an empty
  `@submit.prevent=""`, `v-model="x as T"`, a stray quote in `<template v-else">`) are valid code
  or an ordinary error at the attribute here.
- **A script left open** (a missing `}` at the end of `<script setup>`) is reported at the end
  of the script, where `vue-tsc` reports nothing.

The codegen started from vuejs/language-tools#6170 at `e75a638e` (with the fixes in
[`tools/reference/language-tools.patch`](tools/reference/language-tools.patch)); the snapshots
in `tests/fixtures` record where it now departs from that reference.

## Performance

`vue-tsc --noEmit` (3.3.12, TypeScript 6.0.3) against `tsc --runExternalCode --noEmit`
(TypeScript 7.1.0-dev.20261003.1) with this mapper, on an Apple M-series laptop (wall time, peak
memory of the process tree):

| project | SFCs | vue-tsc | TypeScript 7 + vue-ts-mapper-rs |
| --- | ---: | --- | --- |
| production workspace | 3 571 | 130 s, 13.0 GB | 9.3 s, 6.5 GB |
| production workspace | 2 901 | 65 s, 7.2 GB | 5.5 s, 4.0 GB |
| production workspace | 2 725 | 105 s, 11.3 GB | 7.9 s, 5.8 GB |
| production workspace | 1 931 | 124 s, 12.3 GB | 11.6 s, 6.7 GB |
| production workspace | 903 | 39 s, 3.4 GB | 3.8 s, 2.4 GB |
| halo (console) | 296 | 8.5 s, 1.1 GB | 1.1 s, 0.8 GB |
| vue-pure-admin | 255 | 13.6 s, 1.6 GB | 1.6 s, 1.1 GB |
| element-plus | 164 | 11.0 s, 1.3 GB | 1.9 s, 1.0 GB |
| gitea (web) | 23 | 6.5 s, 1.1 GB | 1.1 s, 0.8 GB |

The mapper process itself peaks at 100–300 MB; the rest is the compiler. On its own, the mapper
transforms about 1 000 SFCs per second on one core (≈1 ms per SFC, 13 KB on average) and scales
across cores.

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
cargo test --release                     # unit tests + snapshots of the language-tools fixtures
UPDATE_FIXTURES=1 cargo test --release --test conformance    # rewrite the snapshots
cargo run --release --example bench -- files.txt 8
cargo run --release --example syntax_scan -- a.vue b.vue …   # SFCs whose virtual code does not parse
cargo run --release --example dump_one -- a.vue [needle]     # virtual code, or mappings around a needle
```

The virtual code of a valid SFC must parse: TypeScript reports syntax errors regardless of the
mapper's directives, and a single one stops the semantic check of the whole program.

To compare with the reference codegen on any set of `.vue` files, build it once
(`tools/reference/build.sh`, needs Node.js and network), dump its output and diff:

```sh
node tools/reference/dump.cjs files.txt > ref.jsonl
cargo run --release --example conformance -- ref.jsonl --out /tmp/diffs
```

## License

MIT. Ported from vuejs/language-tools (MIT, © Johnson Chu); see [LICENSE](LICENSE).
