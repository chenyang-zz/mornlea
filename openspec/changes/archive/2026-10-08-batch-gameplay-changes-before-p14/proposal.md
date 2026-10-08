## Why

Until P14 (`godot-default-client-switch`) switches the default authority to Rust, every gameplay rule change must land in both the Go authority and the Rust port with parity tests. Scattered gameplay changes multiply that paired cost and destabilize the F2 parity baseline.

Source decision: chen via Vera, 2026-10-08. Exception criteria: Lena, 2026-10-08.

## What Changes

- Add one requirement to `development-governance`: before P14, gameplay rule changes are grouped into a planned batch OpenSpec change. Only blocking defects (a hang, item loss, or lost progress) may be fixed in their own change. Unclear cases are decided by the gameplay owner, with the reason recorded in the fix's pull request description. Every gameplay change, batched or blocking, changes the Go authority and the Rust port together and includes Go/Rust parity tests. The rule is removed or replaced once the Rust authoritative server is the default authority.
- Add one pointer sentence to root `AGENTS.md` near the OpenSpec workflow rules.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `development-governance`: Adds the pre-P14 gameplay batching requirement and its blocking-defect exception.

## Impact

- Affected files: `openspec/specs/development-governance/spec.md` (through archive) and root `AGENTS.md`.
- Process only. No code, protocol, save schema, ABI, or performance-contract change.
- No guard test: this is a review-time process rule, not a mechanically checkable repository fact.
- Gameplay owner: Lena at the time of this change. The specification names the role, not the person.
- Rollback: revert the archive commit and the `AGENTS.md` sentence together.
