import { slot } from "@ctx-traits/cdk";

export const scopeNote = slot.text({
  id: "scope-note",
  description: "Short scope note for the topic: the core question, two to four sub-questions, and explicit exclusions.",
});
