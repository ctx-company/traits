// Owner narration for the basic variant, declarative form (owner ruling
// 2026-09-01, superseding 0273's script wrappers): every notification is
// one bare `ctx-notify` argv — no shell, no embedded programs. The begin
// response parses into a typed slot and later steps address
// `notifyId` directly; the review round's rendering is a
// scribe seat emitting a typed digest whose fields feed the commands
// verbatim. MVP by ruling: no failure tolerance — a notifier failure
// fails its step. Receipts stay visible: no --quiet, so each command's
// JSON response lands in the log slot as evidence.
import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import {
  gateAnswer,
  notifyBadge,
  notifyDigest,
  notifyId,
  notifyJournal,
  notifyLog,
  notifySummary,
  report,
  task,
  verdict1,
} from "../data.ts";

export function begin(title: string): void {
  cdk.step.command(title, {
    id: "notify-begin",
    argv: ["ctx-notify", "begin", task],
    output: notifyId,
  });
}

export function update(title: string, status: string): void {
  cdk.step.command(title, {
    id: cdk.idFromTitle(title),
    argv: ["ctx-notify", "update", notifyId, "--status", status],
    output: notifyLog,
  });
}

export function reviewUpdate(title: string): void {
  cdk.step.prompt(`${title}: digest the verdict`, {
    id: "notify-digest-verdict",
    agent: scribe,
    input: cdk.input.prompt`
      Digest this review verdict for the owner's phone: ${verdict1}.
      The worker's report the verdict graded: ${report}.
      Follow each output field's own description exactly; the fields feed
      notification commands verbatim.
    `,
    output: notifyDigest,
  });
  // argv accepts whole slot refs only (never field refs), so one project
  // step fans the typed digest into the three text slots argv can carry.
  cdk.step.project(`${title}: carry the digest`, {
    id: "notify-carry-digest",
    projections: [
      { source: notifyDigest, field: "badge", destination: notifyBadge },
      { source: notifyDigest, field: "summary", destination: notifySummary },
      { source: notifyDigest, field: "journal", destination: notifyJournal },
    ],
  });
  cdk.step.command(title, {
    id: "notify-review-update",
    argv: ["ctx-notify", "update", notifyId, "--status", notifyBadge, "--summary", notifySummary],
    output: notifyLog,
  });
  cdk.step.command(`${title}: journal`, {
    id: "notify-review-journal",
    argv: ["ctx-notify", "log", notifyId, notifyJournal],
    output: notifyLog,
  });
}

export function gateResult(title: string): void {
  cdk.step.command(title, {
    id: "notify-gate-result",
    argv: ["ctx-notify", "log", notifyId, gateAnswer],
    output: notifyLog,
  });
}

export function finish(title: string): void {
  cdk.step.command(title, {
    id: "notify-finish",
    argv: ["ctx-notify", "finish", "--ok", notifyId],
    output: notifyLog,
  });
}
