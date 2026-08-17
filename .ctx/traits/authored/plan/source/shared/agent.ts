import { agent } from "@ctx-traits/cdk";

export const smart1 = (description: string, summary: string) => agent.reviewer("smart-1", { description, summary });
export const smart2 = (description: string, summary: string) => agent.reviewer("smart-2", { description, summary });
