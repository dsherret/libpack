import { instantiate } from "./lib/rs_lib.generated.js";
import * as path from "https://deno.land/std@0.188.0/path/mod.ts";

export interface PackOptions {
  entryPoint: string;
  outputPath: string;
  testPath?: string;
  /** Whether to type check the outputted declaration file.
   * Defaults to `true`.
   */
  typeCheck: boolean;
}

export async function pack(options: PackOptions) {
  const rs = await instantiate();
  const output = await rs.pack({
    entryPoints: [path.toFileUrl(path.resolve(options.entryPoint)).toString()],
  });
  const jsOutputPath = path.resolve(options.outputPath);
  const jsExtName = path.extname(jsOutputPath);
  const dtsOutputPath = jsOutputPath.slice(0, -jsExtName.length) + ".d.ts";
  await Deno.writeTextFileSync(
    jsOutputPath,
    `/// <reference types="./${path.basename(dtsOutputPath)}" />\n` +
    output.js,
  );
  await Deno.writeTextFileSync(dtsOutputPath, output.dts.replaceAll("*/ ", "*/\n"));
  if ((options.typeCheck ?? true) && options.testPath == null) {
    const output = await new Deno.Command(Deno.execPath(), {
      args: ["check", "--no-config", dtsOutputPath],
    }).spawn();
    if (!await output.status) {
      Deno.exit(1);
    }
  }
  if (options.testPath != null) {
    const importMapObj = {
      imports: {
        [path.toFileUrl(path.resolve(options.entryPoint)).toString()]: path.toFileUrl(jsOutputPath).toString(),
      }
    };
    const uri = `data:,${JSON.stringify(importMapObj)}`;
    // todo: configurable permissions
    const args = ["test", "-A", "--import-map", uri];
    if (options.typeCheck === false) {
      args.push("--no-check");
    }
    args.push(options.testPath);
    const output = await new Deno.Command(Deno.execPath(), {
      args,
    }).spawn();
    if (!await output.status) {
      Deno.exit(1);
    }
  }
}
