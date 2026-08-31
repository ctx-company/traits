import { port, slot } from "@ctx-traits/cdk";

export const topic = port.input.text({
  id: "topic",
  description:
    "The idea to brainstorm, in your own words: the feature or capability to explore for this system. Rough is fine.",
});
export const ownerGate = port.input.text({
  id: "owner-gate",
  description:
    "Owner acceptance transport: 'annotate' (default) opens the diagram and pipes the proposal to ctx-annotate; 'off' auto-approves for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

export const slug = slot.text({
  id: "slug",
  description: "Filesystem slug derived from the topic; both artifacts are named by it under .internal/brainstorms/.",
});
export const proposal = slot.text({
  id: "proposal",
  description:
    "The composed proposal receipt: the two artifact paths written (markdown sibling and HTML diagram), the researched sources consulted (URLs), and a one-paragraph summary of the recommended approach.",
});
export const ownerAnswer = slot.text({
  id: "owner-answer",
  description:
    "The owner's verdict from the acceptance gate: the literal string 'approved' ends the loop; any other content is the ctx-annotate decision JSON whose annotations are binding corrections for the next revision.",
});
export const revisionNote = slot.text({
  id: "revision-note",
  description: "What the correction pass changed in both artifacts, per annotation.",
});
export const commitLog = slot.text({
  id: "commit-log",
  description: "Git's own output for the brainstorm artifacts commit.",
});

export const receipts = port.output.text("receipts", {
  description: "The accepted proposal receipt: artifact paths, sources consulted, and the recommended approach.",
  value: proposal,
});
