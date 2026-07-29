import type { AgentHandle, ResourceHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";
import { refactorFrame, survey, target } from "../data.ts";

export function surveyStep(agent: AgentHandle, dialect: ResourceHandle, smells: ResourceHandle) {
    return sequence.prompt("survey", {
        title: "Survey the target (smart-1)", agent,
        text: prompt.text`
            Survey ${target} for refactoring opportunities.
            Read the target and its callers/callees with your tools — explore organically; the friction you encounter while reading IS the signal. Ground every observation in the architecture dialect (${dialect}) and the smell catalog (${smells}); cite smell ids (S1-S10) or a named deep-module violation for each.
            Sweep is mandatory and complete: (a) walk ALL ten smells S1-S10 in catalog order and for each report findings or explicitly "S<n>: none found"; (b) then assess the three dialect pillars — boundaries (missing Interface/Service contracts, mixed control/view/trace responsibilities, surface-coupled layers), data flow (untyped events/commands, string dispatch), entity containment (entity vocabulary leaked across modules).
            Return a numbered candidate list — for each: the cluster of code involved (paths), why it is coupled, which smell ids or dialect pillar it violates, and how the boundary would change what tests can prove.
            Do NOT propose interfaces or fixes yet. List every real candidate even if it cannot fit one run.`,
        output: survey,
    });
}

export function frameStep(agent: AgentHandle, dialect: ResourceHandle, smells: ResourceHandle) {
    return sequence.prompt("frame", {
        title: "Frame the problem (smart-1)", agent,
        text: prompt.text`
            From the survey ${survey} of ${target}, select the highest-value candidate set that honestly fits one refactoring run; name the deferred candidates explicitly so they are not lost.
            Verify the selection against the actual code with your tools, then write the problem framing: the constraints any new boundary must satisfy (callers, serialized shapes that must stay byte-stable, gates), the dependencies involved, and a short illustrative sketch that grounds the space without prescribing the answer.
            Judge with ${dialect} and ${smells}; behavior-preserving is the default contract.`,
        output: refactorFrame,
    });
}
