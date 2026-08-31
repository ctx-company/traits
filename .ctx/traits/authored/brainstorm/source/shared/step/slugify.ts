// Deterministic slug from the topic: lowercase, non-alphanumerics to
// dashes, collapsed and trimmed, capped at 60 chars. Both artifacts are
// named by it so reruns on the same topic overwrite their predecessors.
import * as cdk from "@ctx-traits/cdk";

import { slug, topic } from "../data.ts";

const SLUG_SCRIPT = [
  'printf %s "$1" | tr "[:upper:]" "[:lower:]" | sed -e "s/[^a-z0-9]/-/g" -e "s/--*/-/g" -e "s/^-//" -e "s/-$//" | cut -c1-60',
].join("\n");

export function slugStep(title: string): void {
  cdk.step.command(title, {
    id: "derive-slug",
    argv: ["sh", "-c", SLUG_SCRIPT, "_", topic],
    output: slug,
  });
}
