# Vendored fonts

Both faces are vendored from the `IBM/plex` GitHub repository, pinned to
release tag `v6.4.2`, commit `242c4cccd37e87985a5337815c99b960ef13c65c`
(annotated tag `97d379616be089afcf86880324f18c0408914d9d` dereferences to
that commit). Not a branch URL — a fixed commit.

Static Regular TTFs only, not the variable-font packages some newer IBM
Plex distributions ship as the headline artifact: only weight 400 is
consumed by any call site in this task, so a variable face's weight-axis
matching is an unnecessary unknown.

| file | upstream path | sha256 |
|---|---|---|
| `IBMPlexSans-Regular.ttf` | `IBM-Plex-Sans/fonts/complete/ttf/IBMPlexSans-Regular.ttf` | `975dcda37d80f038dcd143c22e33ca2d97a0cc5a929aace1c749153b0fe1afa5` |
| `IBMPlexSans-Italic.ttf` | `IBM-Plex-Sans/fonts/complete/ttf/IBMPlexSans-Italic.ttf` | `a9c6ef9942c49e49d11e11a6dacc0b3a087978757e9b22a06b8ac22a6400fb15` |
| `IBMPlexMono-Regular.ttf` | `IBM-Plex-Mono/fonts/complete/ttf/IBMPlexMono-Regular.ttf` | `fe11304a5fe956d5744e9b6a246cc83d90425245e75a62230044966ca96a7f50` |
| `IBMPlexMono-Italic.ttf` | `IBM-Plex-Mono/fonts/complete/ttf/IBMPlexMono-Italic.ttf` | `8ebe04c8c6cc82f0be19896ddc61d9935cdd0f027b0173c1945b8d247d7dfc2a` |

`OFL.txt` is the upstream `LICENSE.txt` at the same commit, verbatim — SIL
Open Font License, Version 1.1, with the IBM copyright line.

Mono Italic is vendored for the loop-narration line's inline voice
(`0265.6`, `grammar.md:49-56`) — weight 500 remains reserved for emphasized
in-block titles (sibling-owned) and is still not consumed by any call site.

Sans Italic is vendored for grammar rule 6's screen-header summary
(`0265.13`, `grammar.md:58-66`): a synthesized (CoreText-oblique) sans
italic is a failure, not a fallback, per this file's own doctrine above.
