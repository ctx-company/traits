import { resource } from "@ctx-traits/cdk";

export const architectureDialect = resource({
    id: "architecture-dialect",
    path: "resources/architecture-dialect.md",
    hint: "The house architecture dialect: Interface/Service boundaries, typed Event/Command flow, entity containment — deep modules throughout.",
    trigger: "on-activation",
});

export const smellCatalog = resource({
    id: "smell-catalog",
    path: "resources/smell-catalog.md",
    hint: "Eight detectable refactoring smells with detection patterns, before/after shapes, and when-not-to-apply guards.",
    trigger: "on-activation",
});
