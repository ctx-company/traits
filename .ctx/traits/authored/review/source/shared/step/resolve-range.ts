import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { range } from "../data.ts";

export const rangeResolved = slot.text({
  id: "range-resolved",
  description: "The normalized two-endpoint range actually diffed and logged, after preflight resolution.",
});

export function resolve(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c 'r="$1"; case "$r" in *...*|*..*) resolved="$r" ;; *) git rev-parse --verify "$r" >/dev/null 2>&1 || exit 0; base=$(git merge-base HEAD "$r" 2>/dev/null) || exit 0; resolved="$base..$r" ;; esac; git rev-list "$resolved" >/dev/null 2>&1 && echo "$resolved"; exit 0' _ ${range}`,
    output: rangeResolved,
  });
}
