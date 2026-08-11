import { input, output } from "@ctx-traits/cdk";

import { refactorFrame, survey, target } from "../../data.ts";

export const inputPrompt = input.prompt`
    From the survey ${survey} of ${target}, select the highest-value candidate set that honestly fits one refactoring run; name the deferred candidates explicitly so they are not lost.
    Verify the selection against the actual code with your tools. Judge with the architecture dialect and the smell catalog.
`;

export const outputPrompt = output.prompt`
    Return the problem framing: the selected candidates, the constraints any new boundary must satisfy (callers, serialized shapes that must stay byte-stable, gates), the dependencies involved, and a short illustrative sketch that grounds the space without prescribing the answer. (${refactorFrame})
`;
