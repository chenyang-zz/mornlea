# Control connection retirement fixture implementation plan

> For agentic workers: use superpowers:executing-plans and test-driven-development. tasks.md is the sole status source.

**Goal:** Recognize both EOF and a peer reset as actual Unix control-connection retirement while continuing to reject timeouts and live connections.

**Architecture:** One test-only predicate classifies a completed read. The real activated-process test keeps its dribbling writer, absolute350ms observation bound, join, real shutdown and exclusive-world cleanup. No binary/control/transport behavior or production deadline changes.

**Tech stack:** Existing std::io Result/ErrorKind and UnixStream. **Spec:** ../specs/rust-authoritative-server/spec.md, bounded shutdown requirement. Root cumulative79 log /workspace/scratch/settled-actor-projection-integrated-server.log shows actual ConnectionReset at activation.rs:1861 after connection retirement; read_connection uses an absolute100ms deadline and drops the expired stream. The read EOF-only assertion is the incorrect premise. This repair is independent of79's five-path projection scope.

Exactly ONE editable source/test path: packages/engine/crates/mornlea_server/tests/persistence_failure/activation.rs. Existing crate guide governs the test; no new module/ownership boundary. Binary/activation/provider/store/all other fixtures/seals remain read-only.

Add private control_connection_retired(result:&std::io::Result<usize>)->bool. It is true exactly for Ok(0), or Err whose kind is ConnectionReset. Ok(positive), WouldBlock, TimedOut, BrokenPipe, UnexpectedEof and Other remain false. Keep actual elapsed<350ms, writer.join and owner.shutdown unchanged. Do not accept an arbitrary I/O error, read timeout, longer deadline or omit the actual process test.

Root first adds the predicate with baseline EOF-only behavior and a table test control_retirement_accepts_only_eof_or_reset. Assert EOF and ConnectionReset true, Ok(1) and the six listed non-reset errors false. Compiled behavioral RED must fail at ConnectionReset, separately from the already retained actual full-suite failure. Then implement the typed classification and route only the existing actual dribbled assertion through it. No other control expectations change.

Run exact table test and real control_dribbled_request_obeys_absolute_deadline via owned-child supervisor/pinned previous package/rebuilt Rust binary. Run all48 existing actual activation cases plus the new table case, default threads; root then reruns cumulative server suite, docs, all-target clippy-Dwarnings, fmt/diff, one-path scope/new-English-comment/current-task-ID audit and protected seals. Independent exact-source review may reuse the current isolated79 reviewer after its own five-path review completes, with a separate report and real activation49 gate. This does not change79's accepted source identity or claim full runtime acceptance.

Commit test(server): recognize peer reset as control retirement. Root acceptance closes only3.9l6 after independent review/cumulative actual green; prior226/227 fail remains retained. Rollback reverts only the test classifier/table/assertion. Architecture skill: no change; Unix EOF/reset evidence is fixture-local.
