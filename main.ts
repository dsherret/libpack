import { parse } from "https://deno.land/std@0.190.0/flags/mod.ts";
import * as path from "https://deno.land/std@0.190.0/path/mod.ts";
import { exists } from "https://deno.land/std@0.190.0/fs/exists.ts";
import { pack } from "./mod.ts";

const args = parse(Deno.args, {
  boolean: ["no-deno-json", "no-check", "no-tests"],
  string: ["output-folder", "build-branch", "release-tag-prefix"],
});

const firstArg = args._[0];
if (firstArg === "build") {
  await buildCommand();
} else if (firstArg === "publish") {
  await publishCommand();
} else {
  throw new Error("Unexpected command. Expected 'build' or 'publish'.");
}

async function buildCommand() {
  const rawEntryPoint = args._[1];
  if (typeof rawEntryPoint !== "string") {
    throw new Error(
      "Expected an entry point path to be specified as the first argument to the `build` command.",
    );
  }

  const outputFolder = path.resolve(getArg("output-folder"));
  const entryPoint = path.resolve(rawEntryPoint);
  const entryPointExt = path.extname(entryPoint);
  const entryPointBaseName = path.basename(entryPoint);
  const entryPointNoExt = entryPointBaseName.slice(
    0,
    -1 * entryPointExt.length,
  );

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
}

async function publishCommand() {
  const publishFile = "./publish.ts";
  const module = await import(publishFile);
  const publish: typeof import("./publish.ts").publish = module.publish;
  await publish({
    branch: getArg("build-branch"),
    folder: getArg("output-folder"),
    tagPrefix: getArg("release-tag-prefix"),
    token: getEnv("GITHUB_TOKEN"),
  });
}

function getEnv(name: string): string {
  const env = Deno.env.get(name);
  if (env == null) {
    throw new Error(`Expected environment variable ${name} to be set.`);
  }
  return env;
}

function getArg<T extends keyof typeof args>(
  name: T,
): NonNullable<(typeof args)[T]> {
  const value = args[name];
  if (value == null) {
    throw new Error(`Expected --${name} to be set.`);
  }
  return value;
}
