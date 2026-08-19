// Per-variant intent fragments (0109, mirroring implement's source/core.ts):
// each variant declares its own exact set via useIntent in its hook body —
// the shell does not declare a common intent — because the canonical
// preserves require/avoid declaration order and quick's set is not a
// prefix/suffix-stable subset of default/deep's merged order. Single-trait-
// package rule: these stay here until a second consuming package exists.
import type { Intent } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

/** quick's lean intent set (variant/quick/index.ts). */
export const quick: Intent = {
  require: [intent.Correctness, intent.Leanness, intent.ReviewBeforeFinal],
  avoid: [intent.OverEngineering, intent.ScopeCreep, intent.RubberStampReview],
};

/** default/deep's superset intent (variant/default/index.ts, variant/deep/index.ts). */
export const full: Intent = {
  require: [
    intent.Correctness,
    intent.Robustness,
    intent.Leanness,
    intent.ReviewBeforeFinal,
  ],
  avoid: [
    intent.OverEngineering,
    intent.ScopeCreep,
    intent.UnboundedLoop,
    intent.RubberStampReview,
  ],
};
