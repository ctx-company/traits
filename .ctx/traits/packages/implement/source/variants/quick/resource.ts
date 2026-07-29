import { resource } from "@ctx-traits/cdk";

export const executionPlan = resource({
    id: "execution-plan",
    path: ".plans/EXECUTION_PLAN.md",
    root: "repo",
    hint: "Repo-root path for the execution plan; agents read that file with their own tools and never inline it.",
    trigger: "on-demand",
});

export const productDocument = resource({
    id: "product-doc",
    path: ".docs/PRODUCT.md",
    root: "repo",
    hint: "Repo-root path for the product contract: ADVISORY context, and the only document standing rules may be cited from — no other .docs file (archive/, research/, BLOG_*, LAUNCH_*, or any other) may create a rule. The phase contract outranks it wherever they conflict. Agents read this file with their own tools and never inline it.",
    trigger: "on-demand",
});
