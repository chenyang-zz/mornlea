# Taste learnings
- Follows OpenSpec + subagent-driven-development discipline: planning docs are the single source of truth, changes are proposed as reviewable OpenSpec changes, and agents stay within strict role boundaries (planner plans, implementer implements). Confidence: 0.85
- Git discipline: commits are docs-only and scoped to the run's own files only (never carry user workspace changes or untracked files); two-phase commits with fixed message conventions (`docs: plan <IDs>`, `docs: record planner run <date>`); pushes to `main` must be fast-forward — never force-push (rebase/replay or abort instead). Confidence: 0.85
- Planning docs must never contain dates, estimates, or schedules; dependencies are expressed via row IDs only. Confidence: 0.8
- Version/contract columns record only directional conclusions (e.g. "protocol version bump"), never absolute version numbers. Confidence: 0.8
- Status changes are mirrored to GitHub Discussion via structured comments (`【状态变更】ID name → state`) and script-generated body updates — never hand-edit generated long tables. Confidence: 0.8
- Nothing gets recorded without a source: ideas without a traceable, committed document reference are marked "待澄清" (to clarify) rather than acted on; status corrections leave a written rationale rather than silent deletions. Confidence: 0.8
- Documentation is written in Chinese using the project's established terminology (权威 / 原子 / 有界 / 确定性…); Go/Rust identifiers are wrapped in backticks. Confidence: 0.85
