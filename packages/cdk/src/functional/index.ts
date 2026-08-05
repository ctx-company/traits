/**
 * Public surface of the functional authoring layer (0106): the registration
 * DSL that compiles 1:1 into today's object-layer constructors. Re-exported
 * from `../index.ts` — importing `@ctx-traits/cdk` at all is what installs
 * the `agent.prompt`/`slot.forEach` lowering (`registrars.ts`'s module-init
 * side effect) before any authoring code can call them.
 */
export { attachProcedureFrom } from "./procedure.js";
export type { ProcedureFromFields, ProcedureFromFunction } from "./procedure.js";
export { effect, flow, step } from "./registrars.js";
export type { FieldMatchArms, ForEachRegistrarOptions, GuardMatchArms, LoopParam, ParParam } from "./registrars.js";
