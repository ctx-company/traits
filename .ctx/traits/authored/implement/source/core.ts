// Redistributed into source/shared/{intent,metadata}.ts (0183 step 1: quick).
// Kept as a re-export so the still-legacy variants (default, smart, strict,
// phase, gated, guarded) keep compiling unchanged until they migrate too.
export { FAMILY_BEHAVIOR } from "./shared/metadata.ts";
export { FAMILY_INTENT, GATED_INTENT, QUICK_INTENT, STRICT_INTENT } from "./shared/intent.ts";
