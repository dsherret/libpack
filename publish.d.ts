interface Input {
  folder: string;
  token: string;
  branch: string;
  tagPrefix: string;
  gitUserName?: string;
  gitUserEmail?: string;
}
export function publish(input: Input): Promise<void>;
