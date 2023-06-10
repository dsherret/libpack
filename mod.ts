import { instantiate } from "./lib/rs_lib.generated.js";
import * as path from "https://deno.land/std@0.191.0/path/mod.ts";

export interface PackOptions {
  entryPoint: string;
  outputFolder: string;
  testFile?: string;
  /** Whether to type check the outputted declaration file.
   * Defaults to `true`.
   */
  typeCheck: boolean;
  importMap?: string;
  onDiagnostic?: (diagnostic: Diagnostic) => void;
}

export interface LineAndColumnDisplay {
  lineNumber: string;
  columnNumber: string;
}

export interface Diagnostic {
  specifier: string;
  message: string;
  lineAndColumn: LineAndColumnDisplay | undefined;
}

export function outputDiagnostic(diagnostic: Diagnostic) {
  console.warn(
    `ERROR: ${diagnostic.message} -- ${diagnostic.specifier}${
      formatLineAndColumn(diagnostic.lineAndColumn)
    }`,
  );
}

function formatLineAndColumn(lineAndColumn: LineAndColumnDisplay | undefined) {
  if (lineAndColumn == null) {
    return "";
  }
  return `:${lineAndColumn.lineNumber}:${lineAndColumn.columnNumber}`;
}

export async function pack(options: PackOptions) {
  const rs = await instantiate();
  const importMapUrl = options.importMap == null
    ? undefined
    : path.toFileUrl(path.resolve(options.importMap));
  let diagnosticCount = 0;
  const output: { js: string; dts: string; importMap: string | undefined, hasDefaultExport: boolean } =
    await rs.pack({
      entryPoints: [
        path.toFileUrl(path.resolve(options.entryPoint)).toString(),
      ],
      importMap: importMapUrl?.toString(),
    }, (diagnostic: Diagnostic) => {
      if (options.onDiagnostic) {
        options.onDiagnostic(diagnostic);
      } else {
        diagnosticCount++;
        outputDiagnostic(diagnostic);
      }
    });
  const jsOutputFolder = path.resolve(options.outputFolder);
  const jsOutputPath = path.join(options.outputFolder, "mod.js");
  const tsOutputPath = path.join(options.outputFolder, "mod.ts");
  const dtsOutputPath = path.join(jsOutputFolder, "mod.d.ts");
  await Deno.writeTextFileSync(
    jsOutputPath,
    `/// <reference types="./mod.d.ts" />\n${output.js}`,
  );
  await Deno.writeTextFileSync(
    tsOutputPath,
    (() => {
      let text = `// @deno-types="./mod.d.ts"\nexport * from "./mod.js";\n`;
      if (output.hasDefaultExport) {
        text += `import defaultExport from "./mod.js";\nexport default defaultExport;`
      }
      return text;
    })()
  );
  // todo: https://github.com/swc-project/swc/issues/7492
  await Deno.writeTextFileSync(
    dtsOutputPath,
    output.dts.replaceAll("*/ ", "*/\n"),
  );
  if (diagnosticCount > 0) {
    throw new Error(`Failed. Had ${diagnosticCount} diagnostic${diagnosticCount != 1 ? "s" : ""}.`);
  }
  if ((options.typeCheck ?? true) && options.testFile == null) {
    const checkOutput = await new Deno.Command(Deno.execPath(), {
      args: ["check", "--no-config", tsOutputPath],
    }).spawn();
    if (!await checkOutput.status) {
      Deno.exit(1);
    }
  }
  if (options.testFile != null) {
    const importMapObj = output.importMap == null
      ? {}
      : JSON.parse(output.importMap);
    importMapObj.imports ??= {};
    importMapObj
      .imports[path.toFileUrl(path.resolve(options.entryPoint)).toString()] =
        path.toFileUrl(tsOutputPath).toString();
    // todo: needs to handle scopes
    if (importMapUrl != null) {
      for (const [key, value] of Object.entries(importMapObj.imports)) {
        if ((value as string).startsWith("./")) {
          importMapObj.imports[key] = new URL(value as string, importMapUrl)
            .toString();
        }
      }
    }
    const uri = `data:,${JSON.stringify(importMapObj)}`;
    // todo: configurable permissions
    const args = ["test", "-A", "--import-map", uri];
    if (options.typeCheck === false) {
      args.push("--no-check");
    }
    args.push(options.testFile);
    const testOutput = await new Deno.Command(Deno.execPath(), {
      args,
    }).spawn();
    if (!await testOutput.status) {
      Deno.exit(1);
    }
  }
}
