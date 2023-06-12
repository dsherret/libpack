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
export function outputDiagnostic(diagnostic: Diagnostic): void;
export function pack(options: PackOptions): Promise<void>;
