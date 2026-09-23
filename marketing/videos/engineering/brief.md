# Video: build technical trust in AXTON through engineering design

## Goal

Give developers a reasoned basis for evaluating whether they can depend on AXTON in their applications. Establish trust through concrete mechanisms, verification evidence, design trade-offs, and explicit boundaries.

The viewer should leave thinking: "I understand how the difficult cases are handled, what the guarantees depend on, and how to verify whether this fits my application."

## Audience

Developers who understand AXTON's basic value and want to assess its engineering before adopting it or relying on it more broadly.

## Core message

Reliable local-first behavior depends on coordinating persistent local intent with backend outcomes and incoming state. Explain the chosen mechanisms and their limits using concrete scenarios that viewers can connect to implementation and tests.

## Proposed story

1. **Return to the familiar edit.** Reuse the introduction's collaborative To-do scenario, then introduce a delayed message or an additional local edit.
2. **Expose the relevant state.** Show the distinction between visible local data, pending operations, and authoritative backend state.
3. **Examine a difficult case.** Show where a straightforward implementation gives an undesirable result, then explain AXTON's chosen mechanism.
4. **Discuss the trade-off.** State why the choice was made, what complexity or constraint it introduces, and what responsibilities remain with the application.
5. **Connect the explanation to evidence.** Point to relevant code, meaningful tests or reproducible demonstrations, documented guarantees, and current limits.
6. **Reassemble the flow.** Return to the full synchronization picture and give viewers links for further evaluation.

## Candidate engineering questions

Choose one or two connected questions during scripting so the video has a coherent argument. This is a candidate list, not a requirement to cover every topic:

- Why does a backend acknowledgment not always mean the client can immediately remove its optimistic local changes? What if acknowledgment and synchronized state arrive in either order?
- How does the client incorporate remote state while additional local edits are still pending?
- When a request times out and is retried, how are persistent request identity and backend processing coordinated to avoid duplicate execution within the supported contract?

For each selected question, explain the scenario, the simple approach's failure, the chosen design, its trade-offs, and supporting evidence. Verify current implementation and open correctness issues before choosing claims for the final script.

## Scope boundaries

- Technical trust is the goal; implementation detail is included when it helps viewers assess behavior or a trade-off.
- Avoid repeating the introduction's product pitch or turning this into a complete API tutorial.
- Do not equate conceptual animations with runtime verification, or passing selected tests with universal correctness.
- Explain applicable backend responsibilities, supported conditions, and material unresolved limitations alongside any reliability claim.

## Production and dependencies

- Use Manim to visualize state, message ordering, and the consequence of each design choice.
- Reuse the introduction's scenario and shared visual language. The final videos can be published separately.
- Read claims against `docs/engineering/`, the current implementation, relevant test assertions, and actual verification results. Code inspection alone is not test execution.
- Link the chosen scenario to the mobile To-do demo in [#31](https://github.com/zanminwang/axton/issues/31) where applicable. Do not require the web demo for an explanation of the engine.
- Store production files under `marketing/videos/engineering/`.

## Acceptance criteria

- [ ] The script focuses on one or two connected engineering questions with a clear scenario and consequence.
- [ ] Each selected design is explained together with its rationale, trade-offs, and application responsibilities.
- [ ] Animation accurately distinguishes local visibility, pending operations, and backend authority.
- [ ] Each material correctness or reliability claim has traceable documentation, implementation, and relevant verification evidence; the demonstrated revision is recorded.
- [ ] Known limits and unresolved behavior relevant to the chosen scenarios are represented accurately.
- [ ] Viewers receive links to the architecture, code, and tests or reproducible demonstrations needed for their own evaluation.
- [ ] Final video, captions, scene source, and reproduction instructions are delivered, with a published artifact link recorded in the video index.

The intended outcome is informed technical confidence. A polished animation alone is not evidence that the system is ready for every production setting.
