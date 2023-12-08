# libpack

**DO NOT USE -- Very untested**

Module concatenator for large Deno libraries (prototype).

1. Concatenates your Deno TypeScript library to a single `.js` file with a
   corresponding single `.d.ts` file.
   - JS output somewhat similar to input.
     - Very simple module concatenation. Code is written in such a way that it
       will be easy to switch to
       [module declarations](https://github.com/tc39/proposal-module-declarations)
       in the future once stage 3 and supported in TypeScript.
2. Unfurls your import map in the output—resolves remote module specifiers.
   - Allows you to use [import maps](https://deno.com/manual/basics/import_maps)
     in your library.
3. Type checks the outputted declaration file.
4. Runs integration tests on the outputted JavaScript code using the
   corresponding `x.test.ts` file for the entrypoint (ex. `mod.test.ts` for
   `mod.ts`).

Non goals:

- Minification—instead the output should strive to be human readable.
- Concatenation of remote dependencies and npm packages. These should be left
  external.
- Bundler optimizations.
- General purpose bundler (ex. web bundler or npm package bundler)

## Why?

Performance:

- Type checking only occurs on a single bundled .d.ts file that only contains
  the public API.
- No waterfalling with internal modules within a library because there is only a
  single JS file.
- Compiling of TS to JS is done ahead of time.

## Declaration emit

This emits a single `.d.ts` very quickly by parsing with [swc](https://swc.rs/)
and analyzing explicit types in the public API's only. This is more primitive
than what the TypeScript compiler is capable of, but it is very fast. The tool
will error when something in your public API is not explicitly typed.

The current implementation of this needs a lot more work. It will currently
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

Add a `libpack` and `build` deno task to your _deno.json_ file:

```jsonc
// deno.json
{
  "tasks": {
    "build": "rm -rf dist && deno task libpack build mod.ts && cp README.md dist/README.md",
    "libpack": "deno run -A https://deno.land/x/libpack@{VERSIONGOESHERE}/main.ts --output-folder=dist"
  },
  "imports": {
    "$std/": "https://deno.land/std@0.191.0/"
  },
  "exclude": [
    "dist"
  ]
}
```

Then try it out:

```sh
deno task build
```

This will:

1. Delete the `dist` directory if it exists.
2. Build your library to the `dist` directory using `mod.ts` as an entrypoint,
   type check the output, then run integration tests on the output using
   _mod.test.ts_.

### Publishing

...todo...

With the current deno.land/x registry, publishing is a pain and I'm not going to
bother adding instructions at the moment. There is an upcoming replacement
registry that will solve this issue and once it lands, this library will be
questionable due to "low resolution type checking", but still probably useful
for some use cases.
