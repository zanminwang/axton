# Working on AXTON

AXTON is a schema-driven framework for local-first applications with an application-owned backend. See [README](README.md) for the product overview and [documentation](docs/README.md) for the full index.

## Workspace

- Check the current branch, worktree, and uncommitted changes before editing.
- Work in an isolated worktree; keep changes out of the main checkout. Reuse the current worktree when it already provides isolation.
- Use `codex/` for new branch names unless the task specifies otherwise. Preserve unrelated user changes.

## Engineering

- Read the relevant [architecture](docs/engineering/architecture.md) and [guarantees](docs/engineering/guarantees.md) before changing behavior. Preserve component responsibilities and make contract changes explicit.
- Follow the [testing strategy](docs/engineering/testing/strategy.md) to choose coverage and [running tests](docs/engineering/testing/running.md) for commands and prerequisites.
- Verify the affected behavior before reporting completion. Distinguish tests executed from code inspected, and report material limits in the evidence.

## Documentation

- Follow the [writing conventions](docs/writing/README.md): [architecture](docs/writing/architecture.md), [testing](docs/writing/testing.md), and [guides](docs/writing/guides.md).
- Keep documentation concise and link to the owning source instead of duplicating it. Keep lasting guidance separate from task plans and progress.
- Update affected documentation with behavior changes. Check relative links, heading anchors, and examples when editing or moving documents.
- Maintain repository documentation in English. Follow [website setup](website/README.md) when changing the documentation site.

## Marketing

- Keep positioning, content plans, and production assets under [marketing](marketing/README.md).
- Base product and engineering claims on implemented behavior and supporting evidence. Distinguish available capabilities from planned work.

## Skills

- Consult the [skills index](docs/agents/skills.md) for skills relevant to the task. Read the selected skill's instructions before using it; an index entry does not install or activate a skill.

## Workflows

- Follow [ship an issue](docs/agents/workflows/ship-issue.md) when working on a GitHub issue. A workflow sequences skills from the index, names the labels to set, and says when to write state back to the issue.
