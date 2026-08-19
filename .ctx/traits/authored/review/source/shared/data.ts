import { port } from "@ctx-traits/cdk";

export const range = port.input.text({
  id: "range",
  description:
    'The change to review, as a git range — e.g. "main...HEAD" or "HEAD~3..HEAD". Handed to git as written; git rejects a range it cannot resolve.',
});
