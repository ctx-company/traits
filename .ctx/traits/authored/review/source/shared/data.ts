import { port } from "@ctx-traits/cdk";

export const range = port.input.text({
  id: "range",
  description:
    'A locally-resolvable git ref or range (e.g. "main...feature-x", a branch name, or a single ref meaning <merge-base of default>..ref). Preflight resolves and validates it with git rev-parse/git merge-base before anything else runs.',
});
