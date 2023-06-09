import { pack } from "./mod.ts";
import { parse } from "https://deno.land/std@0.190.0/flags/mod.ts";
import * as path from "https://deno.land/std@0.190.0/path/mod.ts";
import { exists } from "https://deno.land/std@0.190.0/fs/exists.ts";

const args = parse(Deno.args, {
  boolean: ["no-deno-json", "no-check", "no-tests"],
});

if (args._.length !== 2) {
  console.error(
    `Expected 2 arguments (not ${args._.length}). One for the entrypoint and one for the destination folder.`,
  );
  Deno.exit(1);
}
const entryPoint = path.resolve(args._[0] as string);
const outputFolder = path.resolve(args._[1] as string);
const entryPointExt = path.extname(entryPoint);
const entryPointBaseName = path.basename(entryPoint);
const entryPointNoExt = entryPointBaseName.slice(0, -1 * entryPointExt.length);

const testFile = args["no-tests"]
  ? undefined
  : path.join(path.dirname(entryPoint), `${entryPointNoExt}.test.ts`);
if (testFile != null && !await exists(testFile)) {
  console.error(
    `Expected an integration test file at ${testFile}. Run with --no-tests to skip.`,
  );
  Deno.exit(1);
}
const importMap = args["no-deno-json"]
  ? undefined
  : path.join(path.dirname(entryPoint), "deno.json");
if (importMap != null && !await exists(importMap)) {
  console.error(
    `Expected a deno.json file at ${importMap}. Run with --no-deno-json to skip.`,
  );
  Deno.exit(1);
}

await pack({
  entryPoint,
  outputFolder,
  typeCheck: !args["no-check"],
  testFile,
  importMap,
});
