/// <reference types="./publish.d.ts" />
import * as pack0 from "https://deno.land/std@0.191.0/encoding/base64.ts";
import * as pack1 from "https://deno.land/x/dax@0.32.0/mod.ts";
pack1.default.setPrintCommand(true);
export async function publish(input) {
  // Rewritten from: https://github.com/denoland/publish-folder/blob/main/action.yml
  // Copyright (c) 2023 the Deno authors
  const { folder, token, branch, tagPrefix, gitUserName, gitUserEmail } = input;
  const githubRef = getEnvVar("GITHUB_REF");
  const githubEventName = getEnvVar("GITHUB_EVENT_NAME");
  if (githubEventName === "tag") {
    const refTag = githubRef.replace("refs/tags/", "");
    if (refTag.startsWith(tagPrefix)) {
      throw new Error(`Tag '${refTag}' starts with the tag prefix '${tagPrefix}'. ` + `You probably have your workflow configured incorrectly as this step ` + `shouldn't run on tags with the tag prefix.`);
    }
  }
  const currentBranch = await pack1.default`git rev-parse --abbrev-ref HEAD`.text();
  if (currentBranch == branch) {
    throw new Error(`The current branch (${currentBranch}) was the same as the output branch (${branch}). Perhaps you're accidentally copying the GitHub Actions workflow file to the output branch?`);
  }
  const currentSha = await pack1.default`git rev-parse HEAD`.text();
  pack1.default.logStep(`Publishing ${currentSha}`);
  const currentCommitMessage = await pack1.default`git log -1 --pretty=%B`.text();
  pack1.default.logGroup("Commit message", ()=>{
    pack1.default.logLight(`${currentCommitMessage}`);
  });
  const publishDir = await pack1.default`realpath ${folder}`.text();
  pack1.default.logLight(`Publish dir: ${publishDir}`);
  const TEMP_DIR = `${getEnvVar("RUNNER_TEMP")}/deno-x-publish`;
  const USER_NAME = gitUserName ?? "github-actions[bot]";
  const USER_EMAIL = gitUserEmail ?? "github-actions[bot]@users.noreply.github.com";
  pack1.default.logStep(`Creating temp dir ${TEMP_DIR}`);
  await pack1.default`mkdir -p ${TEMP_DIR}`;
  const REPO_URL = `https://github.com/${getEnvVar("GITHUB_REPOSITORY")}/`;
  const AUTH = pack0.encode(`${USER_NAME}:${token}`);
  pack1.default.logStep(`Cloning repo...`);
  pack1.default.cd(TEMP_DIR);
  await pack1.default.raw`git -c http.${REPO_URL}.extraheader="Authorization: Basic ${AUTH}" clone --no-checkout ${REPO_URL} .`;
  pack1.default.logStep(`Setting up repo...`);
  await pack1.default`git config user.name ${USER_NAME}`.text();
  await pack1.default`git config user.email ${USER_EMAIL}`.text();
  await pack1.default.raw`git config http.${REPO_URL}.extraheader "Authorization: Basic ${AUTH}"`;
  const remoteExists = (await pack1.default`git ls-remote --exit-code ${REPO_URL} ${branch}`.noThrow()).code === 0;
  if (remoteExists) {
    await pack1.default`git fetch origin ${branch}`;
    pack1.default.logStep(`Checking out branch ${branch} from ${REPO_URL}...`);
    await pack1.default`git checkout ${branch}`;
  } else {
    pack1.default.logStep(`Creating orphan branch ${branch} for ${REPO_URL}...`);
    await pack1.default`git checkout --orphan ${branch}`;
  }
  await pack1.default.withRetries({
    delay: 2_000,
    count: 5,
    action: async ()=>{
      pack1.default.logStep(`Cleaning repo...`);
      await pack1.default`git rm --ignore-unmatch -rf .`;
      pack1.default.logStep(`Copying files...`);
      await pack1.default`rsync -av --progress ${publishDir}/ ${TEMP_DIR} --exclude '.git'`;
      pack1.default.logStep(`Pushing changes...`);
      await pack1.default`git add .`;
      await pack1.default`git commit --allow-empty -m "Publish ${currentSha}\n\n"${currentCommitMessage}`;
      const result = await pack1.default`git push --set-upstream origin ${branch}`.noThrow();
      if (result.code === 0) {
        return;
      }
      pack1.default.logError(`Push failed. Retrying with the latest changes...`);
      await pack1.default`git fetch origin ${branch}`;
      await pack1.default`git reset --hard origin/${branch}`;
      throw new Error("Failed.");
    }
  });
  if (githubEventName === "tag") {
    const refTag = githubRef.replace("refs/tags/", "");
    const finalTag = `${tagPrefix}${refTag}`;
    pack1.default.logStep(`Publishing tag '${finalTag}'...`);
    await pack1.default`git tag ${finalTag} ${branch}`;
    await pack1.default`git push origin ${finalTag}`;
  } else {
    pack1.default.logLight(`Workflow was not a tag, so not tagging with prefix.`);
  }
}
function getEnvVar(name) {
  const env = Deno.env.get(name);
  if (env == null) {
    throw new Error(`Expected environment variable ${name} to be set.`);
  }
  return env;
}
