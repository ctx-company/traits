import { port, slot } from "@ctx-traits/cdk";

export const topic = port.input.text({
  id: "topic",
  description:
    'The existing functionality, flow, or capability of this system to walk through — named at the product level (e.g. "how a task goes from draft to merged", "the owner acceptance gates"). Rough is fine.',
});
export const ownerGate = port.input.text({
  id: "owner-gate",
  description:
    "Owner acceptance transport: 'annotate' (default) opens the diagram and pipes the walkthrough to ctx-annotate; 'off' auto-approves for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

export const slug = slot.text({
  id: "slug",
  description: "Filesystem slug derived from the topic; both artifacts are named by it under .internal/visualizations/.",
});
export const overview = slot.text({
  id: "overview",
  description:
    "The composed walkthrough receipt: the two artifact paths written (markdown sibling and HTML diagram), the repository areas consulted to establish ground truth, and a one-paragraph summary of the functionality as explained.",
});
export const ownerAnswer = slot.text({
  id: "owner-answer",
  description:
    "The owner's verdict from the acceptance gate: the literal string 'approved' ends the loop; any other content is the ctx-annotate decision JSON whose annotations are binding corrections or deepening requests for the next revision.",
});
export const revisionNote = slot.text({
  id: "revision-note",
  description: "What the revision pass changed or expanded in both artifacts, per annotation.",
});
export const commitLog = slot.text({
  id: "commit-log",
  description: "Git's own output for the visualization artifacts commit.",
});

export const receipts = port.output.text("receipts", {
  description: "The accepted walkthrough receipt: artifact paths, repository areas consulted, and the summary.",
  value: overview,
});
