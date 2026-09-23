# AXTON videos

## Goals

| Video | Audience | Primary goal | Intended next step |
| --- | --- | --- | --- |
| [Introduction](introduction/brief.md) | Developers considering a local-first application | Show the value and developer experience clearly enough to motivate a trial | Try the collaborative To-do demo and follow its quickstart |
| [Engineering](engineering/brief.md) | Developers evaluating whether to depend on AXTON | Establish technical trust through an explanation of mechanisms, evidence, and boundaries | Inspect the implementation and tests, and evaluate fit for their own application |

Adoption is the longer-term outcome. The introduction's immediate goal is a trial; the engineering video's goal is an informed assessment of reliability and trade-offs.

## Production tracking

- [#74 — Introduce AXTON and motivate developers to try it](https://github.com/zanminwang/axton/issues/74).
- [#75 — Build technical trust through AXTON engineering design](https://github.com/zanminwang/axton/issues/75).

## Shared production approach

- Use the same collaborative To-do scenario, participants, and visual vocabulary in both videos.
- Use Manim for explanatory animation. Select and pin the Manim distribution when production begins; Manim Community is the current proposal.
- The introduction gives a short view of how AXTON works. The engineering video expands selected steps into concrete failure scenarios and design reasoning.
- Keep the walkthrough anchored to the mobile demo in [#31](https://github.com/zanminwang/axton/issues/31), implemented as the [To-do example](../../examples/todo/README.md). The later web client in [#72](https://github.com/zanminwang/axton/issues/72) can supply additional footage after it is implemented and verified.
- A schematic animation explains a mechanism; it does not establish that a platform or behavior has been verified. Match capability claims and application footage to the demonstrated revision.
- Store scripts, storyboards, and scene source in each video's directory as production starts. Add `shared/` when there are reusable components or assets.
- Keep rendering caches and large exported video files out of ordinary Git history. Record published video links here when available.

The briefs define goals and scope. Exact duration, narration language, final titles, and release timing are production decisions to make with the scripts.
