# Writing architecture documentation

**Contents**

- [Style](#style)
- [Sections](#sections)
  - [Optional sections](#optional-sections)
- [Notes](#notes)
  - [Component scope](#component-scope)
  - [Splitting components](#splitting-components)
  - [Decisions and risks](#decisions-and-risks)
  - [Verification](#verification)

## Style

- **Clarity:** Give the simplest explanation a reader unfamiliar with the component can understand. Use plain language and explain unfamiliar terms.
- **Precision:** Use concrete wording; avoid vague claims.
- **Consistency:** Use the same term for the same concept.
- **Concision:** Remove repetition while preserving necessary context, causes and exceptions. Ease of understanding matters more than word count.

## Sections

Use the [arc42 template](https://arc42.org/overview/). Its [minimum guidance](https://faq.arc42.org/questions/B-4/) recommends covering core quality requirements, context and interfaces, solution strategy and important decisions, top-level building blocks, and key crosscutting concepts across the system documentation.

Select sections for the system or component being documented.

1. **Introduction and Goals.** Purpose, responsibilities and intended outcomes.
2. **Architecture Constraints.** External requirements that restrict the design.
3. **Context and Scope.** Boundaries, dependencies and external interfaces: operations, inputs and outputs.
4. **Solution Strategy.** The overall approach to meeting the goals.
5. **Building Block View.** Internal components and responsibilities, with code links. For a leaf, identify its implementation without inventing further subdivisions.
6. **Runtime View.** Interactions, state transitions, ordering and recovery. Where child documents do not explain the overall flow, show how the children work together in the parent document.
7. **Deployment View.** Process, device and infrastructure placement.
8. **Crosscutting Concepts.** Mechanisms shared across components; link to their owning document.
9. **Architecture Decisions.** Significant choices, alternatives and consequences.
10. **Quality Requirements.** State the required behavior or invariant first, then link to guarantees or tests. Test names alone do not explain the requirement.
11. **Risks and Technical Debt.** Record each risk's condition, consequence and evidence in its owning component, with relevant issue links.
12. **Glossary.** Terms that need clarification; reuse shared definitions.

### Optional sections

arc42 [does not prescribe a universal required/optional checklist](https://faq.arc42.org/questions/B-1/). AXTON uses these conventions:

- Omit sections that do not apply; do not leave empty headings.
- Keep the original arc42 numbers in headings, such as `## 3. Context and Scope`. Do not renumber after omissions.
- If a relevant section is unresolved, keep its heading and briefly state what needs clarification.

## Notes

### Component scope

- Explain the component's responsibilities, rules and interactions before implementation details. Link to shared explanations and code instead of repeating them.
- Choose the form that makes each explanation easiest to understand: paragraphs, examples, lists, tables, trees or diagrams. Do not force one format across sections or add visuals without a clear purpose.

### Splitting components

- Follow the [component tree](../engineering/architecture.md). It has three levels and no more: AXTON, a component, a part. A part with internal structure keeps its own tree, "how the parts work together" and code map in its README; pages below a part never appear in the top-level tree, graph or code map.
- Keep simple components in one file; split complex components by responsibility.
- Decide what each parent needs to explain. A grouping may need only a brief description and links; when the children leave their collaboration unclear, explain it in the parent's README without repeating their details. Do not require a full document at every level.
- Preserve agreed ownership boundaries. After splitting, update the tree, graph, code map and incoming links.

### Decisions and risks

- Record agreed decisions in the owning component; do not present them as unresolved or invent their rationale.
- Distinguish confirmed problems, potential risks and accepted limitations. A missing feature is not automatically technical debt; do not invent findings to fill section 11.

### Verification

- Verify claims against code. Distinguish current behavior from target design and label uncertainty.
- Distinguish tests read from tests executed. Record commands and results, and preserve the steps or fixtures needed to reproduce material experiments.
