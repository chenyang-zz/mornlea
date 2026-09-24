# Cursor Project Configuration

## Purpose

This directory holds Cursor-specific project settings: plugins, project subagents, and rules that apply to Agent (including Cloud Agents).

## Subagents

Custom subagents live in `.cursor/agents/*.md` (YAML frontmatter + prompt body). They encode Mornlea orchestration roles (implementer, spec reviewer, quality reviewer) for delegation from the main agent.

Cloud Agents load project subagents from the checked-out repository. User-level `~/.cursor/agents/` is not available on cloud VMs.

## Cloud dispatch bridge

Until the Cloud Agent `Task` tool accepts custom `subagent_type` names, follow `.cursor/rules/cloud-project-subagents.mdc`: read the matching agent file and dispatch through the documented built-in `subagent_type` mapping.

## Validation

From the repository root:

```bash
test -d .cursor/agents && ls .cursor/agents/*.md
jq empty .cursor/agents/cloud-task-mapping.json
```
