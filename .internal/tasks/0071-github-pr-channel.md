# 0071 — The `github-pr` channel: the brief becomes the pull request body

**Status:** ready to implement · **Depends on:** 0069; genuinely useful only with 0070 · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Fourth slice of the handoff arc, and the first channel that touches the outside world. Chosen ahead
of Slack deliberately: it needs no secrets of our own, it lands where reviewers already look, and
`story --markdown` was written for exactly this.

## Decisions

- **It is the canonical `Upsert` channel**, keyed on the branch. A second delivery is an update, not
  a duplicate, so a run that lands three more commits refreshes its PR body without any other channel
  re-firing. This is the case that proves `Repeat` is a real property and not a Slack special case.
- **Adoption on first delivery.** With no stored reference, the channel asks `gh` whether a PR
  already exists for this branch and adopts it rather than opening a second. A PR opened by hand is
  the common case and must not produce a duplicate.
- **Draft by default.** A run's PR is a proposal, and opening it ready-for-review notifies
  reviewers about work no human has looked at yet.
- **Our content is fenced.** The brief goes inside an explicitly delimited block so a human editing
  the body around it does not lose their edits on the next update, and so an update never has to
  guess which bytes were ours. Everything outside the fence is preserved verbatim.
- **`gh` is the transport, not a library.** It carries the user's existing auth, which is the whole
  reason this channel needs no secret of ours. Its absence is a `resolve()` failure at `doctor` time
  with the install line, never a runtime surprise.
- **It never merges, never force-pushes, never pushes to a default branch.** The channel writes a
  body and, at most, opens a draft. Landing is the merge path's job and stays there.

## Scope

The `github-pr` channel with `Upsert` + branch key + adoption; `Document`/`Markdown` capabilities
with no budget; the fenced body block with preserve-around semantics; `resolve()` covering `gh`
presence, auth and remote; draft-open behaviour; `--dry-run` printing the exact body and target.

## Watch

- The branch is the key, so a run whose branch was renamed or deleted must fail cleanly and say so,
  not open a PR against something else.
- Repositories with no remote, or a remote that is not GitHub, must resolve to a clear
  "not applicable here" rather than an error — this channel being unusable is a normal condition.
- The body is public to everyone with repo access. Same posture as 0069: paths and links over content
  dumps, no raw transcripts, and `--dry-run` output safe to read before the first real send.
- A parked run still deserves a PR body — a mergeable branch with an honest "parked, here is why" is
  more useful than silence. 0062 makes parked branches mergeable per step, which strengthens this.

## Done when

A completed run opens or adopts a draft PR whose body is the `Document` brief; a second delivery
edits that body in place and preserves human edits outside the fence; a missing `gh`, a missing auth
or a non-GitHub remote is reported by `doctor` rather than at delivery; `--dry-run` prints the body
and target without touching the remote; and nothing in the channel can merge, force-push or write to
a default branch.
