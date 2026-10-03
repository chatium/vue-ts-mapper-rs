#!/bin/sh
# The reference implementation the Rust port is checked against: vuejs/language-tools PR #6170
# (content-mapper, v4) at a pinned commit, plus language-tools.patch (the codegen fixes this port
# also carries). Packs @vue/language-core and @vue/content-mapper and installs them here.
set -eu
cd "$(dirname "$0")"
COMMIT=e75a638eb3252fc25a3d51559c327e0f760cb235
PNPM="npx --yes pnpm@11.9.0"
[ -d work ] || git clone --filter=blob:none "${LT_REPO:-https://github.com/vuejs/language-tools.git}" work
(
	cd work
	git checkout -q --force "$COMMIT"
	git apply ../language-tools.patch
	$PNPM install --frozen-lockfile
	$PNPM run build
	rm -rf ../packs && mkdir ../packs
	(cd packages/language-core && $PNPM pack --pack-destination ../../../packs)
	(cd packages/content-mapper && $PNPM pack --pack-destination ../../../packs)
)
rm -rf node_modules package-lock.json
npm install --no-audit --no-fund
