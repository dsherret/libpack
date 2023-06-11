import { encode } from "$std/encoding/base64.ts"
import $ from "https://deno.land/x/dax@0.32.0/mod.ts";

$.setPrintCommand(true);

interface Input {
  folder: string;
  token: string;
  branch: string;
  tagPrefix: string;
  gitUserName?: string;
  gitUserEmail?: string;
}

export async function publish(input: Input) {
  // rewritten from: https://github.com/denoland/publish-folder/blob/main/action.yml
  // Copyright (c) 2023 the Deno authors
  const {
    folder,
    token,
    branch,
    tagPrefix,
    gitUserName,
    gitUserEmail
  } = input;

  if (Deno.env.get('GITHUB_EVENT_NAME') === 'tag') {
    const refTag = Deno.env.get('GITHUB_REF')?.replace("refs/tags/", "") ?? "";
    if (refTag.startsWith(tagPrefix)) {
      throw new Error(
        `Tag '${refTag}' starts with the tag prefix '${tagPrefix}'. ` +
        `You probably have your workflow configured incorrectly as this step ` +
        `shouldn't run on tags with the tag prefix.`
      );
    }
  }

  const currentBranch = await $`git rev-parse --abbrev-ref HEAD`.text();
  if (currentBranch == branch) {
    throw new Error(`The current branch (${currentBranch}) was the same as the output branch (${branch}). Perhaps you're accidentally copying the GitHub Actions workflow file to the output branch?`);
  }

  const currentSha = await $`git rev-parse HEAD`.text();
  $.logStep(`Publishing ${currentSha}`);

  const publishDir = await $`realpath ${folder}`.text();
  $.logLight(`Publish dir: ${publishDir}`);

  const TEMP_DIR = `${Deno.env.get('RUNNER_TEMP')}/deno-x-publish`;
  const USER_NAME = gitUserName ?? 'github-actions';
  const USER_EMAIL = gitUserEmail ?? 'github-actions@github.com';

  $.logStep(`Creating temp dir ${TEMP_DIR}`);
  await $`mkdir -p ${TEMP_DIR}`;

  const REPO_URL = `https://github.com/${Deno.env.get('GITHUB_REPOSITORY')}/`;
  const AUTH = encode(`${USER_NAME}:${token}`);

  $.logStep(`Cloning repo...`);
  $.cd(TEMP_DIR);
  await $`git -c http.${REPO_URL}.extraheader="Authorization: Basic ${AUTH}" clone --no-checkout ${REPO_URL} .`;

  $.logStep(`Setting up repo...`);
  await $`git config user.name ${USER_NAME}`.text();
  await $`git config user.email ${USER_EMAIL}`.text();
  await $`git config http.${REPO_URL}.extraheader "Authorization: Basic ${AUTH}"`;

  const remoteBranch = await $`git ls-remote --exit-code ${REPO_URL} ${branch}`.text();
  if (remoteBranch) {
    await $`git fetch origin ${branch}`;
    $.logStep(`Checking out branch ${branch} from ${REPO_URL}...`);
    await $`git checkout ${branch}`;
  } else {
    $.logStep(`Creating orphan branch ${branch} for ${REPO_URL}...`);
    await $`git checkout --orphan ${branch}`;
  }

  await $.withRetries({
    delay: 200,
    count: 5,
    action: async () => {

      $.logStep(`Cleaning repo...`);
      await $`git rm --ignore-unmatch -rf .`;

      $.logStep(`Copying files...`);
      await $`rsync -av --progress ${publishDir}/ ${TEMP_DIR} --exclude '.git'`;

      $.logStep(`Pushing changes...`);
      await $`git add .`;
      await $`git commit --allow-empty -m "Publish ${currentSha}"`;

      const result = await $`git push --set-upstream origin ${branch}`.noThrow();
      if (result.code === 0) {
        return;
      }

      $.logError(`Push failed. Retrying with the latest changes...`);
      await $`git fetch origin ${branch}`;
      await $`git reset --hard origin/${branch}`;
      throw new Error("Failed.");
    }
  });

  if (Deno.env.get('GITHUB_EVENT_NAME') === 'tag') {
    const refTag = (await $`echo ${Deno.env.get('GITHUB_REF')}`.text()).replace("refs/tags/", "");
    const finalTag = `${tagPrefix}${refTag}`;
    $.logStep(`Publishing tag '${finalTag}'...`);
    await $`git tag ${finalTag} ${branch}`;
    await $`git push origin ${finalTag}`;
  } else {
    $.logLight(`Workflow was not a tag, so not tagging with prefix.`);
  }
}