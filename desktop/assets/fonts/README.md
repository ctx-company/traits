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
| `IBMPlexMono-Regular.ttf` | `IBM-Plex-Mono/fonts/complete/ttf/IBMPlexMono-Regular.ttf` | `fe11304a5fe956d5744e9b6a246cc83d90425245e75a62230044966ca96a7f50` |

`OFL.txt` is the upstream `LICENSE.txt` at the same commit, verbatim — SIL
Open Font License, Version 1.1, with the IBM copyright line.

Only Sans Regular and Mono Regular are vendored — italic is narration
(`0265.6`) and weight 500 is reserved for emphasized in-block titles
(sibling-owned); neither is consumed by any call site this task repaints.
