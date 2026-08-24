import * as cdk from "@ctx-traits/cdk";

import { targetFile, taskCheck, taskSnapshot } from "../data.ts";

/** Captures the target's content checksum without allowing a prompt to infer it. */
export function capture(title: string): void {
  cdk.step.command(title, {
    input: cdk.input.command`sh -c 'test -f "$1" && cksum < "$1"' _ ${targetFile}`,
    output: taskSnapshot,
  });
}

/** Fails before a write or after criticism when another actor changed the target. */
export function assertUnchanged(title: string): void {
  cdk.step.check(title, {
    argv: [
      "sh",
      "-c",
      'test -f "$1" && cksum < "$1" | grep -qxF -- "$2"',
      "_",
      targetFile,
      taskSnapshot,
    ],
    output: taskCheck,
  });
  cdk.flow.errorWhen(`${title} failed`, cdk.condition.isFalse(taskCheck.ok), {
    message: "the resolved task file is missing or changed since its last architect snapshot; aborting without overwriting it",
  });
}
