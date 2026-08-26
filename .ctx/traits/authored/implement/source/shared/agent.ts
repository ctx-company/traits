import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

export const worker = cdk.agent.worker("worker", {
  description: "Implementation Work",
  summary: "Implementation role.",
  intent: {
    require: [cdk.intent.PreserveScope, cdk.intent.ReviewBeforeFinal],
  },
});

export const smart = cdk.agent.reviewer("smart", {
  description: "Plan Drafting and Review",
  summary: "Review role.",
  intent: {
    avoid: [
      cdk.intent.TasteOnlyBlocking,
      cdk.intent.TailCasePursuit,
      cdk.intent.RubberStampReview,
      cdk.intent.UnboundedLoop,
    ],
  },
});

export const scribe = agents.scribeRole("scribe", "Commit & Summary Writing");
