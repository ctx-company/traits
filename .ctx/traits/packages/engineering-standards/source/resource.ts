import { resource } from "@ctx-traits/cdk";

export const codingStandards = resource({
    id: "coding-standards",
    path: "resources/coding-standards.md",
    hint: "Shared engineering standards every reviewed change is held to; agents read this file with their own tools and never inline it.",
    trigger: "on-activation",
});

export const reviewGuidance = resource({
    id: "review-guidance",
    path: "resources/review-guidance.md",
    hint: "How a reviewer separates blocking defects from advisory notes; agents read this file with their own tools.",
    trigger: "on-activation",
});
