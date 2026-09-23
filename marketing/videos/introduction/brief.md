# Video: introduce AXTON and motivate developers to try it

## Goal

Help developers recognize AXTON's value for their application and take the first step toward trying it. Eventual adoption is the longer-term outcome; the immediate desired action is to try the collaborative To-do demo and follow its quickstart.

The viewer should leave thinking: "This is an experience I want to build, I understand what AXTON handles and what I write, and I know how to try it."

## Audience

Developers interested in responsive applications, offline use, and synchronization who have not yet evaluated AXTON in depth.

## Core message

AXTON provides schema-driven local operations and coordinates local state with background synchronization. Developers use generated interfaces and retain responsibility for their own backend business logic and data reads.

## Proposed story

1. **Show the application experience.** Follow two participants in the collaborative To-do demo. Show an immediate local edit, work while disconnected, and synchronization after reconnection.
2. **Introduce the developer problem.** Explain the work involved in connecting durable local changes to backend processing and updates from other devices. Stay with the concrete scenario.
3. **Explain how AXTON works briefly.** Use Manim to follow one edit through local SQLite, the pending queue, the application backend, and returning state. Establish a useful mental model without expanding protocol details.
4. **Show the developer experience.** Connect a small schema and generated client call to the backend's Handler, Loader, and Notify responsibilities. Make clear which work AXTON handles and which remains application code.
5. **Invite a trial.** End with one primary action: try the demonstrated To-do example. Its quickstart provides the path to running or building with it.

## Scope boundaries

- Keep experience, basic operation, and developer experience in one narrative serving the trial goal.
- Detailed acknowledgment ordering, replay, retry semantics, and design trade-offs belong in the engineering video.
- A full installation tutorial and broad comparisons with other sync products are outside this video.
- A separate general-purpose "Why local-first" video is outside this production scope.

## Production and dependencies

- Use Manim for explanatory animation, with demo footage or clearly presented application views for the user experience.
- Reuse the collaborative mobile To-do demo in [#31](https://github.com/zanminwang/axton/issues/31). Plan animation and script before the demo is ready; verify final footage and claims against its actual behavior before publication.
- The web demo in [#72](https://github.com/zanminwang/axton/issues/72) is optional and does not block the mobile-based narrative.
- Store production files under `marketing/videos/introduction/`. Reuse visual components with the engineering video when practical.
- Keep the first script concise; duration is provisional until narration and storyboard are reviewed.

## Acceptance criteria

- [ ] Script and storyboard communicate the application value, developer value, and next step.
- [ ] A viewer can explain what AXTON handles and what application developers still implement.
- [ ] A short Manim sequence traces one edit through the local and backend flow accurately.
- [ ] Developer-experience examples use current APIs and correspond to the demonstrated scenario.
- [ ] Demo behavior, platform claims, and the shown revision have been checked; planned capabilities are not presented as available.
- [ ] The final call to action points to a usable demo entry point and its quickstart.
- [ ] Final video, captions, scene source, and reproduction instructions are delivered, with a published artifact link recorded in the video index.

Production completion is distinct from campaign effectiveness. Evaluate whether viewers take the intended trial step where measurement is available; do not treat a finished render as evidence of adoption.
