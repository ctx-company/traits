import { agent } from "@ctx-traits/cdk";

export const smart = (description: string, summary: string) => agent.reviewer("smart", { description, summary });
