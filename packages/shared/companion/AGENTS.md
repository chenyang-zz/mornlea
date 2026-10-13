# Companion domain package

`packages/shared/companion` defines companion domain identity, dialogue trees,
persona models, planning tools, task queues, terrain projection, snapshot
registries, and JSON contract mappings for the companion agent subsystem. It
belongs to the `packages/shared` module and may only import `packages/shared/core`.
Production code must not import server authority, client rendering, or tools.
In particular, `packages/shared/companion` and `packages/tools/cmd/runtime-oracle`
must not import each other, enforced by `packages/audit` `TestInternalDependenciesAreOneWay`.

## Agent contract serialization and oracle (`agent_contract_json.go`, `runtime_contract_oracle_test.go`)

- `agent_contract_json.go` implements serialization and deserialization for
  companion agent HTTP and MCP contract schemas.
- `runtime_contract_oracle_test.go` verifies contract serialization against the
  frozen corpus under `testdata/runtime-migration`. Every test run performs
  read-only byte-for-byte comparisons against frozen input and expected files.
- Test flags or code branches capable of modifying `testdata/runtime-migration`
  are strictly prohibited.
- Fixture generation is optional and strictly decoupled from the tracked corpus.
  It is gated by the `RUNTIME_ORACLE_EXPORT_DIR` environment variable. When unset,
  no exports are written. When set, `exportGeneratedAssetsFromEnvironment` delegates
  to `exportGeneratedAssets` with the fixed producer ID `companion/agent-contract`.
- The export helper verifies that the export root's nearest existing ancestor is
  strictly outside the repository and contains no symlinks. It validates all
  asset paths, duplicates, and file-as-parent collisions before mutation, then
  creates export-root gaps, producer prefixes, and asset parents one component
  at a time with `Lstat`/`Mkdir`. Every existing component must be a real
  directory; the final fixed producer child
  (`<exportRoot>/companion/agent-contract`) is created exclusively and any
  preexisting final child is rejected. Resolved containment is checked again
  before assets are opened with `os.O_EXCL|os.O_CREATE|os.O_WRONLY`, preventing
  symlink redirection, path traversal, or replacement.
- Enforcement: `TestAgentContractCorpusRoundTrip`, `TestCompanionExportGeneratedAssets`.

## Focused Verification

```bash
go test ./packages/shared/companion -race -count=1
go test ./packages/shared/companion -run '^Test(AgentContractCorpus|CompanionExport)' -count=1
make archcheck
```
