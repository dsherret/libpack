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

Performance. Large libraries with many files or deeply nested modules can
improve their loading times and especially type checking times by being
distributed as a single JS file with a separate corresponding declaration file.

## Declaration emit

This emits a single `.d.ts` very quickly by parsing with [swc](https://swc.rs/)
and analyzing explicit types in the public API's only. This is more primitive
than what the TypeScript compiler is capable of, but it is very fast. The tool
will error when something in your public API is not explicitly typed.

The current implementation of this needs a lot more work, but it will currently
error when it finds something not supported.
