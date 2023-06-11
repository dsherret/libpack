# lib_pack

**DO NOT USE -- Very untested and doesn't work well with `deno doc`**

Module concatenator for large Deno libraries (prototype).

1. Concatenates your Deno TypeScript library to a single `.js` file with a
   corresponding single `.d.ts` file.
2. Unfurls your import map—resolves remote module specifiers.
3. Type checks the outputted declaration file.
4. Runs integration tests on the outputted JavaScript code using the
   corresponding `x.test.ts` file for the entrypoint (ex. `mod.test.ts` for
   `mod.ts`).

Features:

- Output somewhat similar to input.
  - Very simple module concatenation. Code is written in such a way that it will
    be easy to switch to
    [module declarations](https://github.com/tc39/proposal-module-declarations)
    in the future once stage 3 and supported in TypeScript.
- Allows you to use [import maps](https://deno.com/manual/basics/import_maps) in
  your library.

Non goals:

- Minification—instead the output should strive to be human readable.
- Concatenation of remote dependencies and npm packages. These should be left
  external.
- Bundler optimizations.
- General purpose bundler or application bundler to a single file.

## Why?

1. Performance.
  - Type checking only occurs on a single bundled .d.ts file
  - No waterfalling with internal modules within a library because there is only
    a single JS file.
  - Compiling of TS to JS is done ahead of time.
2. Private APIs stay private.
  - People can't import your internal modules. Only what's exported from the main entrypoint is available.

## Declaration emit

This emits a single `.d.ts` very quickly by parsing with [swc](https://swc.rs/)
and analyzing explicit types in the public API's only. This is more primitive
than what the TypeScript compiler is capable of, but it is very fast. The tool
will error when something in your public API is not explicitly typed.

The current implementation of this needs a lot more work, but it will currently
error when it finds something not supported.

## Usage

### Building

Add a `.gitignore` to your project and ignore the `dist` directory, which we'll
be using for the build output:

```
dist
```

Set up your library with a _mod.ts_ file. For example:

```ts
// mod.ts
export function add(a: number, b: number): number {
  return a + b;
}
```

Add a corresponding integration test file at _mod.test.ts_:

```ts
// mod.test.ts
import { assertEquals } from "$std/testing/asserts.ts";
import { add } from "./mod.ts";

Deno.test("adds numbers", () => {
  assertEquals(add(1, 2), 3);
});
```

Add a `build` deno task to your _deno.json_ file:

```js
{
  "tasks": {
    "build": "rm -rf dist && deno task lib_pack build mod.ts"
    "lib_pack": "deno run -A https://deno.land/x/lib_pack@{VERSIONGOESHERE}/main.ts --output-folder=dist"
  },
  "imports": {
    "$std/": "https://deno.land/std@0.191.0/" // or use a newer version
  },
  "exclude": ["dist"]
}
```

Then run:

```sh
deno task build
```

This will:

1. Delete the `dist` directory if it exists.
2. Build your library to the `dist` directory using `mod.ts` as an entrypoint,
   type check the output, then run integration tests on the output using
   _mod.test.ts_.

### Publishing

Publishing works by:

1. We tag our repo with a version number. For example: `1.0.0`.
1. A GH Actions Workflow is triggered for that tag.
   1. It builds the output to a `dist` directory.
   1. It pushes the `dist` directory to a separate orphaned `build` branch.
   1. It tags the `build` branch with a `release/` prefix tag.

### Setup

1. **Update your repository webhook's payload URL** for https://api.deno.land to
   have a `version_prefix` query parameter of `release/` in Settings > Webhooks:

   ```
   https://api.deno.land/webhook/gh/{your_module_name}?version_prefix=release/
   ```

   This will cause deno.land/x to only publish when the workflow tags the
   `build` branch with a `release/` prefix.

2. Create a `.github/workflows/ci.yml` file in your repository with content
   similar to the following:

   ```yml
   # .github/workflows/ci.yml
   name: ci

   on:
     push:
       branches: ["main"]
       tags: ["!release/**"]
     pull_request:
       branches: ["main"]

   jobs:
     deno:
       runs-on: ubuntu-latest

       steps:
         - uses: actions/checkout@v3
         - uses: denoland/setup-deno@v1

         - name: Lint
           run: deno lint

         - name: Test
           run: deno test -A

         - name: Build
           run: deno task build

         - name: Push to build branch and release if tag
           if: github.ref == 'refs/heads/main'
           env:
             GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
           run: deno task lib_pack publish --publish-branch=build --release-tag-prefix=release/
   ```
