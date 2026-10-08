## 1. Gameplay batching rule

- [x] 1.1 Add the gameplay batching requirement to `development-governance` through this change's ADDED delta; validate with `openspec validate batch-gameplay-changes-before-p14 --strict`.
- [x] 1.2 Add one sentence to root `AGENTS.md` near the OpenSpec workflow rules pointing to the `development-governance` specification.
- [x] 1.3 Run `openspec validate --all --strict --no-interactive`, `go test ./packages/audit -count=1`, and `git diff --check`.
- [x] 1.4 Archive the change in the same pull request.
