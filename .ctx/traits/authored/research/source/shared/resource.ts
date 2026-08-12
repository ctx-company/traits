import { resource } from "@ctx-traits/cdk";

export const researchStandards = resource.file("research-standards", {
  path: "resources/research-standards.md",
  hint: "Canonical flat-layout doctrine, source-rating scale, citation policy, and Chain-of-Verification method for this family.",
  trigger: "on-demand",
});
export const sourceQualityGuide = resource.file("source-quality-guide", {
  path: "resources/source-quality-guide.md",
  hint: "Domain examples and a decision method for applying the canonical A-E source-quality scale.",
  trigger: "on-demand",
});
export const citationStyle = resource.file("citation-style", {
  path: "resources/citation-style.md",
  hint: "Source-type citation formats and verification rules.",
  trigger: "on-demand",
});
