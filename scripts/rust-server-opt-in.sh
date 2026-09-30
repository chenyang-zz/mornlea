#!/usr/bin/env bash
# Opt-in Rust server activation and rollback.
#
# The script qualifies one disposable world copy for the opt-in Rust server
# without touching default startup: `activate` guards, backs up, and starts
# the named Rust binary, while `rollback` stops Rust, proves quiescence and
# the world lock, then restarts the named previous binary under either the
# compatible or the restore-backup data policy. `--self-test` runs the real
# disposable workflows plus a separate dry-run inspection. Structured work
# (JSON, hashes, control RPC, lock probes, login handshakes) runs in
# `python3` stdlib helpers; `bash` owns CLI parsing and process control.
# The script requires a Unix host with `bash`, `python3`, and `git` (the
# latter only for the default-startup assertion inside `--self-test`).
set -uo pipefail

SCRIPT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

# Frozen baseline identities from the version matrix. Manifest claims must
# equal these before any rollback starts the previous runtime.
BASE_PROTOCOL=45
BASE_PLAYER_SCHEMA=9
BASE_CHUNK_SCHEMA=9
BASE_WORLD_METADATA_SCHEMA=6
BASE_COMPANIONS_SCHEMA=5
BASE_HOSTILE_SCHEMA=2
BASE_PASSIVE_SCHEMA=1

MANIFEST_BASENAME="activation-manifest.json"
CONTROL_BASENAME="control.sock"
LOCK_BASENAME="world.lock"
BACKUP_IDENTITY_BASENAME=".mcgo-world-backup-v1.json"
# The lock file carries ownership, not durable bytes, so it stays outside
# every tree digest; the backup identity describes the copy rather than the
# world. Backup trees skip both names, world trees skip only the lock, and
# the two digests then agree byte for byte on identical world content.
WORLD_SKIP="world.lock"
BACKUP_SKIP="world.lock,.mcgo-world-backup-v1.json"
SHUTDOWN_DEADLINE_MS=15000
READY_TIMEOUT_SECS=30
GO_READY_TIMEOUT_SECS=60

fail() {
    local code="$1"
    shift
    echo "FAIL ${code} $*" >&2
    exit 1
}

usage() {
    cat >&2 <<'USAGE'
usage: rust-server-opt-in.sh activate --world <disposable-copy> --backup <named-copy> --run-dir <explicit> --rust-bin <path> --previous-bin <path> --previous-sha256 <hex> [--dry-run]
       rust-server-opt-in.sh rollback --manifest <path> --data-policy compatible|restore-backup
       rust-server-opt-in.sh --self-test
       rust-server-opt-in.sh --help
USAGE
    exit 2
}

need_python() {
    command -v python3 >/dev/null 2>&1 || fail "activation_failed" "python3 is required"
    python3 - <<'EOF' || fail "activation_failed" "unix python3 stdlib (fcntl) is required"
import sys
if __name__ == "__main__":
    if getattr(__import__("os"), "name", "") != "posix":
        sys.exit(1)
    __import__("fcntl")
EOF
}

py() {
    python3 - "$@"
}

# Canonicalizes one path; refuses when resolution fails.
canon() {
    py "$1" <<'EOF'
import os, sys
print(os.path.realpath(sys.argv[1]))
EOF
}

# Refuses a leaf symlink alias. Confinement runs on canonical paths, so an
# alias can never point outside the disposable root; refusing the leaf
# keeps the manifest binding exactly the directory the operator named.
reject_symlink_alias() {
    local path="$1"
    py "$path" <<'EOF' || fail "invalid_manifest" "symlink alias in $1"
import os, sys
if os.path.islink(sys.argv[1]):
    sys.exit(1)
EOF
}

sha256_file() {
    py "$1" <<'EOF'
import hashlib, sys
with open(sys.argv[1], "rb") as handle:
    print(hashlib.sha256(handle.read()).hexdigest())
EOF
}

fresh_nonce() {
    py <<'EOF'
import os
print(os.urandom(16).hex())
EOF
}

# Content hash over a world tree: regular files sorted by `/`-joined
# relative path, each contributing path bytes, one zero byte, the
# little-endian content length, then the content bytes. The lock basename
# and the backup identity basename describe ownership rather than world
# bytes and stay outside the digest.
tree_hash() {
    local root="$1"
    local skip="$2"
    py "$root" "$skip" <<'EOF'
import hashlib, os, struct, sys
root, skip = sys.argv[1], set(sys.argv[2].split(","))
entries = []
for current, _dirs, files in os.walk(root):
    for name in files:
        full = os.path.join(current, name)
        if os.path.islink(full) or not os.path.isfile(full):
            print("symlink or special file inside hashed tree: %s" % full, file=sys.stderr)
            sys.exit(3)
        rel = os.path.relpath(full, root).replace(os.sep, "/")
        entries.append((rel, full))
digest = hashlib.sha256()
for rel, full in sorted(entries):
    if os.path.basename(rel) in skip:
        continue
    with open(full, "rb") as handle:
        content = handle.read()
    digest.update(rel.encode("utf-8"))
    digest.update(b"\x00")
    digest.update(struct.pack("<Q", len(content)))
    digest.update(content)
print(digest.hexdigest())
EOF
}

manifest_get() {
    py "$1" "$2" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    manifest = json.load(handle)
value = manifest.get(sys.argv[2])
if value is None:
    print("", end="")
elif isinstance(value, bool):
    print("true" if value else "false", end="")
elif isinstance(value, (dict, list)):
    print(json.dumps(value, sort_keys=True), end="")
else:
    print(value, end="")
EOF
}

# Atomically replaces the manifest record with rendered JSON text.
manifest_put() {
    local manifest="$1"
    local text="$2"
    py "$manifest" "$text" <<'EOF'
import os, sys
path, text = sys.argv[1], sys.argv[2]
staged = "%s.tmp.%d" % (path, os.getpid())
with open(staged, "w") as handle:
    handle.write(text)
    handle.flush()
    os.fsync(handle.fileno())
os.replace(staged, path)
parent = os.open(os.path.dirname(os.path.abspath(path)), os.O_RDONLY)
try:
    os.fsync(parent)
finally:
    os.close(parent)
EOF
}

# Writes one JSON field into the manifest record atomically.
manifest_set_field() {
    local manifest="$1"
    local field="$2"
    local value_json="$3"
    py "$manifest" "$field" "$value_json" <<'EOF'
import json, sys
path, field, raw = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path) as handle:
    manifest = json.load(handle)
manifest[field] = json.loads(raw)
staged = "%s.tmp.set" % path
with open(staged, "w") as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write("\n")
    handle.flush()
    import os
    os.fsync(handle.fileno())
import os
os.replace(staged, path)
EOF
}

# Probes the OS world lock without taking ownership: prints FREE when no
# writer holds it and LOCKED otherwise. A missing lock file means no owner.
lock_probe() {
    py "$1" <<'EOF'
import fcntl, os, sys
path = os.path.join(sys.argv[1], "world.lock")
try:
    fd = os.open(path, os.O_RDONLY)
except FileNotFoundError:
    print("FREE")
else:
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print("LOCKED")
    else:
        fcntl.flock(fd, fcntl.LOCK_UN)
        print("FREE")
    finally:
        os.close(fd)
EOF
}

# Sends one JSON request over the local control socket and prints the
# one-line JSON reply. Prints CONTROL_DOWN when no owner answers.
control_rpc() {
    local socket="$1"
    local request="$2"
    py "$socket" "$request" <<'EOF'
import json, socket as sockmod, sys
path, request = sys.argv[1], sys.argv[2]
client = sockmod.socket(sockmod.AF_UNIX, sockmod.SOCK_STREAM)
client.settimeout(15)
try:
    client.connect(path)
except OSError:
    print("CONTROL_DOWN")
else:
    client.sendall(request.encode("utf-8") + b"\n")
    chunks = []
    while not chunks or not chunks[-1].endswith(b"\n"):
        piece = client.recv(4096)
        if not piece:
            break
        chunks.append(piece)
    print(b"".join(chunks).decode("utf-8").strip())
finally:
    client.close()
EOF
}

# Copies a complete world directory beside its destination, writes the
# named-backup identity, syncs every file and directory, then renames the
# copy into place and syncs the parent. Never touches the live world.
backup_copy() {
    local source="$1"
    local destination="$2"
    local tree="$3"
    py "$source" "$destination" "$tree" <<'EOF'
import json, os, shutil, sys
source, destination, tree = sys.argv[1], sys.argv[2], sys.argv[3]
parent = os.path.dirname(os.path.abspath(destination))
temporary = "%s.tmp.%d" % (destination, os.getpid())
if os.path.lexists(temporary):
    shutil.rmtree(temporary, ignore_errors=True)
shutil.copytree(source, temporary, symlinks=False)
identity = {
    "source": os.path.abspath(source),
    "seed": -1,
    "migration_version": 1,
    "created_by": "rust-server-opt-in",
    "tree_sha256": tree,
}
with open(os.path.join(temporary, ".mcgo-world-backup-v1.json"), "w") as handle:
    json.dump(identity, handle, indent=2, sort_keys=True)
    handle.write("\n")
def synced_walk(top):
    for current, dirs, files in os.walk(top):
        for name in files:
            fd = os.open(os.path.join(current, name), os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        for name in dirs:
            fd = os.open(os.path.join(current, name), os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
synced_walk(temporary)
top_fd = os.open(temporary, os.O_RDONLY)
try:
    os.fsync(top_fd)
finally:
    os.close(top_fd)
os.rename(temporary, destination)
parent_fd = os.open(parent, os.O_RDONLY)
try:
    os.fsync(parent_fd)
finally:
    os.close(parent_fd)
EOF
}

# Verifies the named backup: directory, identity record, and tree digest.
backup_verify() {
    local backup="$1"
    local expect_tree="$2"
    [ -d "$backup" ] || fail "backup_mismatch" "backup directory is absent"
    [ -f "$backup/$BACKUP_IDENTITY_BASENAME" ] || fail "backup_mismatch" "backup identity is absent"
    py "$backup" <<'EOF' || fail "backup_mismatch" "backup identity does not decode"
import json, sys
with open("%s/.mcgo-world-backup-v1.json" % sys.argv[1]) as handle:
    identity = json.load(handle)
if identity.get("migration_version") != 1 or not identity.get("source"):
    sys.exit(1)
EOF
    local actual
    actual="$(tree_hash "$backup" "$BACKUP_SKIP")" || fail "backup_mismatch" "backup tree does not hash"
    [ "$actual" = "$expect_tree" ] || fail "backup_mismatch" "backup tree diverges from the manifest"
}

# Performs the real client login handshake and prints the authoritative
# world seed. Any rejection or timeout fails the verification.
login_seed() {
    local host="$1"
    local port="$2"
    local name="$3"
    py "$host" "$port" "$name" <<'EOF'
import os, socket, struct, sys
host, port, name = sys.argv[1], int(sys.argv[2]), sys.argv[3]
def uvarint(value):
    out = bytearray()
    while True:
        byte = value & 0x7f
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)
def read_uvarint(conn):
    value, shift = 0, 0
    while True:
        byte = conn.recv(1)
        if not byte:
            raise RuntimeError("truncated length prefix")
        byte = byte[0]
        value |= (byte & 0x7f) << shift
        if not byte & 0x80:
            return value
        shift += 7
def read_frame(conn):
    remaining = read_uvarint(conn)
    body = b""
    while len(body) < remaining:
        piece = conn.recv(remaining - len(body))
        if not piece:
            raise RuntimeError("truncated frame body")
        body += piece
    packet_id, offset = 0, 0
    shift = 0
    while True:
        byte = body[offset]
        offset += 1
        packet_id |= (byte & 0x7f) << shift
        if not byte & 0x80:
            break
        shift += 7
    return packet_id, body[offset:]
def write_frame(conn, packet_id, payload):
    body = uvarint(packet_id) + payload
    conn.sendall(uvarint(len(body)) + body)
identity = bytearray(os.urandom(16))
identity[6] = (identity[6] & 0x0f) | 0x40
identity[8] = (identity[8] & 0x3f) | 0x80
conn = socket.create_connection((host, port), timeout=10)
conn.settimeout(10)
write_frame(conn, 0, uvarint(45))
packet_id, payload = read_frame(conn)
if packet_id != 0:
    raise RuntimeError("handshake rejected with packet %d" % packet_id)
version, consumed = 0, 0
shift = 0
for byte in payload:
    consumed += 1
    version |= (byte & 0x7f) << shift
    if not byte & 0x80:
        break
    shift += 7
if consumed != len(payload) or version != 45:
    raise RuntimeError("handshake version mismatch")
name_bytes = name.encode("utf-8")
start = bytes(identity) + uvarint(len(name_bytes)) + name_bytes + bytes((8,))
write_frame(conn, 0, start)
packet_id, payload = read_frame(conn)
if packet_id != 0 or len(payload) != 24:
    raise RuntimeError("login rejected with packet %d" % packet_id)
if bytes(payload[:16]) != bytes(identity):
    raise RuntimeError("login echoes another identity")
print(struct.unpack("<Q", payload[16:])[0])
EOF
}

# Confines every managed path inside the disposable run root, which itself
# must live inside a system temporary tree. Rejects symlink aliases.
confine_paths() {
    local world="$1"
    local backup="$2"
    local run_dir="$3"
    local run_canon
    run_canon="$(canon "$run_dir")"
    local inside_root=0
    local candidate tmp_canon
    for candidate in "${TMPDIR:-/tmp}" "/tmp" "/var/tmp"; do
        tmp_canon="$(canon "$candidate" 2>/dev/null)" || continue
        case "$run_canon" in
            "$tmp_canon"/*) inside_root=1 ;;
        esac
    done
    [ "$inside_root" = "1" ] || fail "invalid_manifest" "run directory escapes the disposable root"
    local world_canon
    world_canon="$(canon "$world")"
    local backup_canon
    backup_canon="$(canon "$backup")"
    case "$world_canon" in
        "$run_canon"/*) ;;
        *) fail "invalid_manifest" "world path escapes the disposable root" ;;
    esac
    case "$backup_canon" in
        "$run_canon"/*) ;;
        *) fail "invalid_manifest" "backup path escapes the disposable root" ;;
    esac
    [ "$world_canon" != "$backup_canon" ] || fail "invalid_manifest" "world and backup coincide"
    [ "$world_canon" != "$run_canon" ] || fail "invalid_manifest" "world coincides with the run directory"
    reject_symlink_alias "$world"
    reject_symlink_alias "$backup"
    reject_symlink_alias "$run_dir"
}

# Reads the bound game address back from the previous runtime startup log.
# Port zero removes the probe-then-bind race: the OS assigns the loopback
# port and the server prints the bound `listen=` address before serving.
# The log appends across restarts, so the last match names the current
# owner rather than a dead predecessor.
previous_listen_from_log() {
    local log="$1"
    py "$log" <<'EOF'
import re, sys
with open(sys.argv[1], errors="replace") as handle:
    text = handle.read()
matches = re.findall(r"listen=([0-9.]+:[0-9]+|\[[0-9a-fA-F:]+\]:[0-9]+)", text)
print(matches[-1] if matches else "")
EOF
}

manifest_require() {
    local manifest="$1"
    [ -f "$manifest" ] || fail "invalid_manifest" "manifest is absent"
    py "$manifest" <<'EOF' || fail "invalid_manifest" "manifest does not decode or name schema 1"
import json, sys
with open(sys.argv[1]) as handle:
    manifest = json.load(handle)
if manifest.get("schema_version") != 1:
    sys.exit(1)
for field in ("start_nonce", "world_path", "backup_path", "control_socket",
              "executable_sha256", "previous_executable", "previous_sha256",
              "world_tree_sha256", "backup_tree_sha256", "phase"):
    if manifest.get(field) is None:
        sys.exit(1)
EOF
}

check_baseline_identities() {
    local manifest="$1"
    [ "$(manifest_get "$manifest" "protocol_version")" = "$BASE_PROTOCOL" ] || fail "incompatible_save" "protocol identity diverges"
    local schemas
    schemas="$(manifest_get "$manifest" "save_schemas")"
    for pair in "player:$BASE_PLAYER_SCHEMA" "chunk:$BASE_CHUNK_SCHEMA" "world_metadata:$BASE_WORLD_METADATA_SCHEMA" "companions_ai:$BASE_COMPANIONS_SCHEMA" "hostile_mobs:$BASE_HOSTILE_SCHEMA" "passive_mobs:$BASE_PASSIVE_SCHEMA"; do
        local family="${pair%%:*}"
        local version="${pair##*:}"
        py "$schemas" "$family" "$version" <<'EOF' || fail "incompatible_save" "save schema diverges"
import json, sys
schemas = json.loads(sys.argv[1])
if schemas.get(sys.argv[2]) != int(sys.argv[3]):
    sys.exit(1)
EOF
    done
}

cmd_activate() {
    local world="" backup="" run_dir="" rust_bin="" previous_bin="" previous_sha256="" dry_run=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --world) world="$2"; shift 2 ;;
            --backup) backup="$2"; shift 2 ;;
            --run-dir) run_dir="$2"; shift 2 ;;
            --rust-bin) rust_bin="$2"; shift 2 ;;
            --previous-bin) previous_bin="$2"; shift 2 ;;
            --previous-sha256) previous_sha256="$2"; shift 2 ;;
            --dry-run) dry_run=1; shift ;;
            -h|--help) usage ;;
            *) usage ;;
        esac
    done
    [ -n "$world" ] && [ -n "$backup" ] && [ -n "$run_dir" ] && [ -n "$rust_bin" ] && [ -n "$previous_bin" ] && [ -n "$previous_sha256" ] || usage
    need_python
    [ -f "$rust_bin" ] && [ -x "$rust_bin" ] || fail "invalid_manifest" "rust binary is absent"
    [ -f "$previous_bin" ] || fail "invalid_manifest" "previous binary is absent"
    confine_paths "$world" "$backup" "$run_dir"
    [ -d "$world" ] || fail "invalid_manifest" "world directory is absent"
    [ -f "$world/world.meta" ] || fail "invalid_manifest" "world carries no stored metadata"
    local actual_previous
    actual_previous="$(sha256_file "$previous_bin")" || fail "invalid_manifest" "previous binary does not hash"
    [ "$actual_previous" = "$previous_sha256" ] || fail "identity_mismatch" "previous binary diverges from the named hash"
    local rust_hash
    rust_hash="$(sha256_file "$rust_bin")" || fail "invalid_manifest" "rust binary does not hash"

    mkdir -p "$run_dir" || fail "invalid_manifest" "run directory is not writable"
    rust_bin="$(canon "$rust_bin")"
    previous_bin="$(canon "$previous_bin")"
    local run_canon world_canon backup_canon
    run_canon="$(canon "$run_dir")"
    world_canon="$(canon "$world")"
    backup_canon="$(canon "$backup")"
    local manifest="$run_canon/$MANIFEST_BASENAME"
    local socket="$run_canon/$CONTROL_BASENAME"
    local nonce=""
    local fresh_run=0
    local adopted_tree=""
    if [ -f "$manifest" ]; then
        manifest_require "$manifest"
        local recorded_rust
        recorded_rust="$(manifest_get "$manifest" "executable_sha256")"
        [ "$recorded_rust" = "$rust_hash" ] || fail "identity_mismatch" "rust binary diverges from the manifest"
        [ "$(manifest_get "$manifest" "world_path")" = "$world_canon" ] || fail "identity_mismatch" "manifest names another world"
        [ "$(manifest_get "$manifest" "control_socket")" = "$socket" ] || fail "identity_mismatch" "manifest names another control socket"
        nonce="$(manifest_get "$manifest" "start_nonce")"
        [ -n "$nonce" ] || fail "invalid_manifest" "manifest carries no start nonce"
        case "$(manifest_get "$manifest" "phase")" in
            Prepared|RustRunning|StopRequested|Quiescent)
                local reply
                reply="$(control_rpc "$socket" '{"op":"status"}')" || fail "activation_failed" "control probe refused"
                if [ "$reply" != "CONTROL_DOWN" ]; then
                    local live_nonce live_phase
                    live_nonce="$(py "$reply" <<'EOF'
import json, sys
print(json.loads(sys.argv[1]).get("nonce", ""))
EOF
)"
                    live_phase="$(py "$reply" <<'EOF'
import json, sys
print(json.loads(sys.argv[1]).get("phase", ""))
EOF
)"
                    if [ "$live_nonce" = "$nonce" ] && [ "$live_phase" = "RustRunning" ]; then
                        echo "OK already active manifest=$manifest phase=RustRunning"
                        return 0
                    fi
                    fail "writer_live" "control socket answers for another owner"
                fi
                [ "$(lock_probe "$world_canon")" = "FREE" ] || fail "writer_live" "world lock is already owned"
                ;;
            DataVerified|PreviousRunning)
                fail "writer_live" "manifest already belongs to the previous runtime"
                ;;
            *) fail "invalid_manifest" "manifest names an unknown phase" ;;
        esac
    else
        fresh_run=1
        nonce="$(fresh_nonce)"
        if [ -e "$backup_canon" ]; then
            local current
            adopted_tree="$(tree_hash "$backup_canon" "$BACKUP_SKIP")" || fail "backup_mismatch" "pre-existing backup does not hash"
            current="$(tree_hash "$world_canon" "$LOCK_BASENAME")" || fail "invalid_manifest" "world does not hash"
            [ "$adopted_tree" = "$current" ] || fail "backup_mismatch" "pre-existing backup diverges from the world"
        fi
    fi

    [ "$(lock_probe "$world_canon")" = "FREE" ] || fail "writer_live" "world lock is already owned"

    if [ "$dry_run" = "1" ]; then
        local probe_manifest="$manifest"
        local scratch_manifest=""
        if [ ! -f "$manifest" ]; then
            scratch_manifest="$(py "${TMPDIR:-/tmp}" <<'EOF'
import os, sys
fd, path = __import__("tempfile").mkstemp(prefix="optin-manifest-", dir=sys.argv[1])
os.close(fd)
print(path)
EOF
)"
            py "$scratch_manifest" "$world_canon" "$nonce" "$rust_hash" <<'EOF'
import json, sys
path, world, nonce, rust_hash = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
with open(path, "w") as handle:
    json.dump({"schema_version": 1, "start_nonce": nonce, "world_path": world,
               "phase": "Prepared", "executable_sha256": rust_hash}, handle)
EOF
            probe_manifest="$scratch_manifest"
        fi
        local probe_socket="${TMPDIR:-/tmp}/optin-probe-$$.sock"
        local dry_output dry_code
        dry_output="$("$rust_bin" --world "$world_canon" --listen "127.0.0.1:0" --activation-manifest "$probe_manifest" --control-socket "$probe_socket" --dry-run 2>&1)"
        dry_code=$?
        [ -n "$scratch_manifest" ] && rm -f "$scratch_manifest"
        [ "$dry_code" = "0" ] || fail "activation_failed" "rust dry-run refused: $dry_output"
        echo "DRYRUN rust_bin=$rust_bin rust_sha256=$rust_hash previous_bin=$previous_bin previous_sha256=$previous_sha256 world=$world_canon backup=$backup_canon run_dir=$run_canon nonce=$nonce"
        echo "$dry_output"
        echo "DRYRUN complete: no manifest, backup, or writer was created"
        return 0
    fi

    local backup_tree=""
    if [ ! -e "$backup_canon" ]; then
        local world_tree
        world_tree="$(tree_hash "$world_canon" "$LOCK_BASENAME")" || fail "invalid_manifest" "world does not hash"
        backup_copy "$world_canon" "$backup_canon" "$world_tree" || fail "backup_mismatch" "backup copy refused"
        backup_tree="$world_tree"
    elif [ "$fresh_run" = "1" ]; then
        backup_tree="$adopted_tree"
    else
        backup_verify "$backup_canon" "$(manifest_get "$manifest" "backup_tree_sha256")"
        backup_tree="$(manifest_get "$manifest" "backup_tree_sha256")"
    fi
    local world_tree
    world_tree="$(tree_hash "$world_canon" "$LOCK_BASENAME")" || fail "invalid_manifest" "world does not hash"

    if [ ! -f "$manifest" ]; then
        py "$manifest" "$world_canon" "$backup_canon" "$socket" "$nonce" "$rust_hash" "$previous_bin" "$previous_sha256" "$world_tree" "$backup_tree" <<'EOF'
import json, sys, time
(path, world, backup, socket, nonce, rust_hash, prev_bin, prev_hash, world_tree, backup_tree) = sys.argv[1:11]
manifest = {
    "schema_version": 1,
    "runtime": "rust",
    "source_sha": "0000000000000000000000000000000000000000",
    "executable_sha256": rust_hash,
    "previous_executable": prev_bin,
    "previous_sha256": prev_hash,
    "protocol_version": 45,
    "save_schemas": {"player": 9, "chunk": 9, "world_metadata": 6,
                     "companions_ai": 5, "hostile_mobs": 2, "passive_mobs": 1},
    "world_path": world,
    "backup_path": backup,
    "world_tree_sha256": world_tree,
    "backup_tree_sha256": backup_tree,
    "control_socket": socket,
    "pid": None,
    "start_nonce": nonce,
    "lease_identity": None,
    "phase": "Prepared",
    "restore_stage": None,
    "last_error": None,
    "created_unix": int(time.time()),
}
with open(path, "w") as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write("\n")
EOF
        manifest_put "$manifest" "$(cat "$manifest")"
    else
        manifest_set_field "$manifest" "world_tree_sha256" "\"$world_tree\""
        manifest_set_field "$manifest" "phase" '"Prepared"'
    fi
    # Records the worktree source identity beside the manifest fields; the
    # manifest keeps the deploy-time provenance the binary cannot observe.
    local source_sha
    source_sha="$(git -C "$(dirname "$SCRIPT_PATH")" rev-parse HEAD 2>/dev/null || true)"
    if [ -n "$source_sha" ]; then
        manifest_set_field "$manifest" "source_sha" "\"$source_sha\""
    fi

    "$rust_bin" --world "$world_canon" --listen "127.0.0.1:0" --activation-manifest "$manifest" --control-socket "$socket" >>"$run_canon/rust-server.log" 2>&1 &
    local rust_pid=$!
    local deadline
    deadline=$((SECONDS + READY_TIMEOUT_SECS))
    local ready=0
    while [ $SECONDS -lt $deadline ]; do
        if ! kill -0 "$rust_pid" 2>/dev/null; then
            wait "$rust_pid" 2>/dev/null
            fail "activation_failed" "rust binary exited before readiness (see rust-server.log)"
        fi
        local reply
        reply="$(control_rpc "$socket" '{"op":"status"}')" || true
        if [ -n "$reply" ] && [ "$reply" != "CONTROL_DOWN" ]; then
            local live_nonce live_phase
            live_nonce="$(py "$reply" <<'EOF' || true
import json, sys
try:
    print(json.loads(sys.argv[1]).get("nonce", ""))
except Exception:
    pass
EOF
)"
            live_phase="$(py "$reply" <<'EOF' || true
import json, sys
try:
    print(json.loads(sys.argv[1]).get("phase", ""))
except Exception:
    pass
EOF
)"
            if [ "$live_nonce" = "$nonce" ] && [ "$live_phase" = "RustRunning" ]; then
                ready=1
                break
            fi
        fi
        sleep 1
    done
    if [ "$ready" != "1" ]; then
        kill -9 "$rust_pid" 2>/dev/null || true
        wait "$rust_pid" 2>/dev/null || true
        fail "activation_failed" "rust binary never reported readiness"
    fi
    [ "$(manifest_get "$manifest" "phase")" = "RustRunning" ] || fail "activation_failed" "rust binary never recorded its phase"
    echo "OK active manifest=$manifest phase=RustRunning pid=$rust_pid"
}

# Stops the Rust owner named by the manifest and proves quiescence plus a
# free world lock. Prints the lock-free world path on success.
stop_rust_owner() {
    local manifest="$1"
    local world="$2"
    local socket="$3"
    local nonce="$4"
    local phase
    phase="$(manifest_get "$manifest" "phase")"
    case "$phase" in
        PreviousRunning)
            return 0
            ;;
        Prepared|RustRunning|StopRequested|Quiescent|DataVerified)
            ;;
        *) fail "invalid_manifest" "manifest names an unknown phase" ;;
    esac
    local last_code
    last_code="$(py "$(manifest_get "$manifest" "last_error")" <<'EOF'
import json, sys
try:
    print(json.loads(sys.argv[1]).get("code", ""))
except Exception:
    print("")
EOF
)"
    [ "$last_code" != "shutdown_failed" ] || fail "shutdown_failed" "a failed shutdown retains its phase"
    local reply
    reply="$(control_rpc "$socket" '{"op":"status"}')" || fail "shutdown_failed" "control probe refused"
    if [ "$reply" != "CONTROL_DOWN" ]; then
        local live_nonce
        live_nonce="$(py "$reply" <<'EOF'
import json, sys
print(json.loads(sys.argv[1]).get("nonce", ""))
EOF
)"
        [ "$live_nonce" = "$nonce" ] || fail "writer_live" "control socket answers for another owner"
        reply="$(control_rpc "$socket" '{"op":"shutdown","deadline_ms":'"$SHUTDOWN_DEADLINE_MS"'}')" || fail "shutdown_failed" "shutdown request refused"
        local shut_phase shut_error
        shut_phase="$(py "$reply" <<'EOF'
import json, sys
try:
    print(json.loads(sys.argv[1]).get("phase", ""))
except Exception:
    print("")
EOF
)"
        shut_error="$(py "$reply" <<'EOF'
import json, sys
try:
    print(json.loads(sys.argv[1]).get("error", ""))
except Exception:
    print("")
EOF
)"
        [ -z "$shut_error" ] || fail "shutdown_failed" "rust shutdown reported $shut_error"
        [ "$shut_phase" = "Quiescent" ] || fail "shutdown_failed" "rust shutdown never quiesced"
        local pid
        pid="$(manifest_get "$manifest" "pid")"
        if [ -n "$pid" ]; then
            local deadline
            deadline=$((SECONDS + SHUTDOWN_DEADLINE_MS / 1000 + 10))
            while [ $SECONDS -lt $deadline ]; do
                kill -0 "$pid" 2>/dev/null || break
                sleep 1
            done
            kill -0 "$pid" 2>/dev/null && fail "shutdown_failed" "rust process never exited"
        fi
        [ "$(manifest_get "$manifest" "phase")" = "Quiescent" ] || fail "shutdown_failed" "quiescent phase was never recorded"
    else
        [ "$(lock_probe "$world")" = "FREE" ] || fail "writer_live" "world lock is already owned"
    fi
    [ "$(lock_probe "$world")" = "FREE" ] || fail "writer_live" "world lock never released"
    echo "$world"
}

verify_lease_identity() {
    local manifest="$1"
    local world="$2"
    local recorded
    recorded="$(manifest_get "$manifest" "lease_identity")"
    [ -n "$recorded" ] || return 0
    local current
    current="$(py "$world" <<'EOF'
import os, sys
info = os.stat(os.path.join(sys.argv[1], "world.lock"))
print("world.lock:%d:%d" % (info.st_dev, info.st_ino))
EOF
)" || fail "identity_mismatch" "lock file is unreadable"
    [ "$current" = "$recorded" ] || fail "identity_mismatch" "lock file was replaced"
}

# Starts the manifest's previous binary on a free loopback port, verifies a
# real login plus sole lock ownership, and records the running phase.
start_previous_owner() {
    local manifest="$1"
    local world="$2"
    local run_canon="$3"
    local previous="$4"
    manifest_set_field "$manifest" "phase" '"DataVerified"'
    "$previous" --world "$world" --listen "127.0.0.1:0" >>"$run_canon/previous-server.log" 2>&1 &
    local pid=$!
    local listen=""
    local deadline
    deadline=$((SECONDS + GO_READY_TIMEOUT_SECS))
    while [ $SECONDS -lt $deadline ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            wait "$pid" 2>/dev/null
            fail "previous_start_failed" "previous binary exited before readiness (see previous-server.log)"
        fi
        listen="$(previous_listen_from_log "$run_canon/previous-server.log")"
        [ -n "$listen" ] && break
        sleep 1
    done
    [ -n "$listen" ] || {
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        fail "previous_start_failed" "previous binary never reported its listen address"
    }
    echo "$listen" >"$run_canon/previous-listen.addr"
    local seed=""
    deadline=$((SECONDS + GO_READY_TIMEOUT_SECS))
    while [ $SECONDS -lt $deadline ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            wait "$pid" 2>/dev/null
            fail "previous_start_failed" "previous binary exited before readiness (see previous-server.log)"
        fi
        if seed="$(login_seed "${listen%%:*}" "${listen##*:}" "optin-verify" 2>/dev/null)"; then
            [ -n "$seed" ] && break
            seed=""
        fi
        sleep 1
    done
    [ -n "$seed" ] || {
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        fail "previous_start_failed" "previous binary never admitted login"
    }
    echo "$seed" >"$run_canon/previous-seed.txt.tmp"
    if [ -f "$run_canon/previous-seed.txt" ]; then
        local expected_seed
        expected_seed="$(cat "$run_canon/previous-seed.txt")"
        if [ "$seed" != "$expected_seed" ]; then
            kill -9 "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
            fail "incompatible_save" "previous runtime serves another seed"
        fi
    fi
    mv "$run_canon/previous-seed.txt.tmp" "$run_canon/previous-seed.txt"
    [ "$(lock_probe "$world")" = "LOCKED" ] || {
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        fail "previous_start_failed" "previous binary never took the world lock"
    }
    manifest_set_field "$manifest" "pid" "$pid"
    manifest_set_field "$manifest" "phase" '"PreviousRunning"'
    echo "OK previous pid=$pid listen=$listen seed=$seed"
}

# Installs the verified backup over the world with the resumable two-rename
# protocol: stage beside the world, retire the current directory, install
# the staged copy, then verify the installed bytes. Every step fsyncs its
# parent so a crash resumes from the recorded stage instead of guessing.
restore_backup_world() {
    local manifest="$1"
    local world="$2"
    local backup="$3"
    local nonce="$4"
    local staged="$world.restore.$nonce"
    local retired="$world.retired.$nonce"
    local backup_tree
    backup_tree="$(manifest_get "$manifest" "backup_tree_sha256")"
    backup_verify "$backup" "$backup_tree"
    local stage
    stage="$(manifest_get "$manifest" "restore_stage")"
    case "$stage" in
        "")
            rm -rf "$staged" "$retired"
            backup_copy "$backup" "$staged" "$backup_tree" || fail "restore_failed" "staging copy refused"
            [ "$(tree_hash "$staged" "$BACKUP_SKIP")" = "$backup_tree" ] || fail "restore_failed" "staged copy diverges"
            manifest_set_field "$manifest" "restore_stage" '"staged"'
            ;;
        staged)
            [ -d "$staged" ] || fail "restore_failed" "staged copy vanished"
            [ "$(tree_hash "$staged" "$BACKUP_SKIP")" = "$backup_tree" ] || fail "restore_failed" "staged copy diverges"
            if [ -e "$retired" ]; then
                if [ -e "$world" ]; then
                    fail "restore_failed" "retired and current worlds collide"
                fi
                manifest_set_field "$manifest" "restore_stage" '"old_retired"'
            fi
            ;;
        old_retired)
            if [ ! -d "$staged" ]; then
                [ -d "$retired" ] || fail "restore_failed" "retired world vanished"
                if [ ! -e "$world" ]; then
                    mv "$retired" "$world" || fail "restore_failed" "retired world does not reinstall"
                    py "$(canon "$(dirname "$world")")" <<'EOF' || fail "restore_failed" "parent does not sync"
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
try:
    os.fsync(fd)
finally:
    os.close(fd)
EOF
                fi
                [ -e "$world" ] || fail "restore_failed" "world path never returned"
                rm -rf "$staged"
                backup_copy "$backup" "$staged" "$backup_tree" || fail "restore_failed" "staging copy refused"
                [ "$(tree_hash "$staged" "$BACKUP_SKIP")" = "$backup_tree" ] || fail "restore_failed" "staged copy diverges"
                manifest_set_field "$manifest" "restore_stage" '"staged"'
            fi
            ;;
        backup_installed)
            ;;
        *) fail "invalid_manifest" "manifest names an unknown restore stage" ;;
    esac
    stage="$(manifest_get "$manifest" "restore_stage")"
    if [ "$stage" = "staged" ] || [ "$stage" = "old_retired" ]; then
        rm -f "$staged/$BACKUP_IDENTITY_BASENAME"
        py "$staged" <<'EOF' || fail "restore_failed" "staged directory does not sync"
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
try:
    os.fsync(fd)
finally:
    os.close(fd)
EOF
    fi
    if [ "$stage" = "staged" ]; then
        [ -e "$world" ] || fail "restore_failed" "current world vanished before retire"
        mv "$world" "$retired" || fail "restore_failed" "current world does not retire"
        py "$(canon "$(dirname "$world")")" <<'EOF' || fail "restore_failed" "parent does not sync"
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
try:
    os.fsync(fd)
finally:
    os.close(fd)
EOF
        manifest_set_field "$manifest" "restore_stage" '"old_retired"'
    fi
    stage="$(manifest_get "$manifest" "restore_stage")"
    if [ "$stage" = "old_retired" ]; then
        [ -d "$staged" ] || fail "restore_failed" "staged copy vanished"
        [ -e "$world" ] && fail "restore_failed" "world path returned before install"
        mv "$staged" "$world" || fail "restore_failed" "staged copy does not install"
        py "$(canon "$(dirname "$world")")" <<'EOF' || fail "restore_failed" "parent does not sync"
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
try:
    os.fsync(fd)
finally:
    os.close(fd)
EOF
        manifest_set_field "$manifest" "restore_stage" '"backup_installed"'
    fi
    [ "$(tree_hash "$world" "$LOCK_BASENAME")" = "$backup_tree" ] || fail "restore_failed" "installed world diverges from the backup"
    local new_lease
    new_lease="$(py "$world" <<'EOF'
import os, sys
info = os.stat(os.path.join(sys.argv[1], "world.lock"))
print("world.lock:%d:%d" % (info.st_dev, info.st_ino))
EOF
)" || fail "restore_failed" "installed lock is unreadable"
    manifest_set_field "$manifest" "lease_identity" "\"$new_lease\""
    manifest_set_field "$manifest" "world_tree_sha256" "\"$backup_tree\""
    RESTORE_RETIRED="$retired"
}

cmd_rollback() {
    local manifest="" policy=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --manifest) manifest="$2"; shift 2 ;;
            --data-policy) policy="$2"; shift 2 ;;
            -h|--help) usage ;;
            *) usage ;;
        esac
    done
    [ -n "$manifest" ] || usage
    case "$policy" in
        compatible|restore-backup) ;;
        *) usage ;;
    esac
    need_python
    manifest_require "$manifest"
    local manifest_canon
    manifest_canon="$(canon "$manifest")"
    local world backup socket nonce
    world="$(manifest_get "$manifest" "world_path")"
    backup="$(manifest_get "$manifest" "backup_path")"
    socket="$(manifest_get "$manifest" "control_socket")"
    nonce="$(manifest_get "$manifest" "start_nonce")"
    local run_canon
    run_canon="$(canon "$(dirname "$manifest_canon")")"
    confine_paths "$world" "$backup" "$run_canon"
    local previous recorded_previous
    previous="$(manifest_get "$manifest" "previous_executable")"
    recorded_previous="$(manifest_get "$manifest" "previous_sha256")"
    [ -f "$previous" ] || fail "identity_mismatch" "previous binary is absent"
    [ "$(sha256_file "$previous")" = "$recorded_previous" ] || fail "identity_mismatch" "previous binary diverges from the manifest"

    local phase resumed_previous=0
    phase="$(manifest_get "$manifest" "phase")"
    if [ "$phase" = "PreviousRunning" ]; then
        if [ "$(lock_probe "$world")" = "LOCKED" ]; then
            local listen_file="$run_canon/previous-listen.addr"
            if [ -f "$listen_file" ]; then
                local host port
                host="$(cut -d: -f1 <"$listen_file")"
                port="$(cut -d: -f2 <"$listen_file")"
                if login_seed "$host" "$port" "optin-resume" >/dev/null 2>&1; then
                    echo "OK already previous manifest=$manifest phase=PreviousRunning"
                    return 0
                fi
            fi
            fail "writer_live" "world lock is already owned"
        fi
        resumed_previous=1
    else
        stop_rust_owner "$manifest" "$world" "$socket" "$nonce" >/dev/null
        # The lease and tree bindings prove an untouched world for the
        # compatible policy. Restore replaces the world directory itself,
        # so it verifies the backup and the installed bytes instead; the
        # previous runtime's own lock acquisition is the final ownership
        # proof on that path.
        if [ "$policy" = "compatible" ]; then
            verify_lease_identity "$manifest" "$world"
        fi
    fi

    local retired=""
    if [ "$resumed_previous" = "0" ] || [ "$policy" = "restore-backup" ]; then
        case "$policy" in
            compatible)
                check_baseline_identities "$manifest"
                local expect_tree actual_tree
                expect_tree="$(manifest_get "$manifest" "world_tree_sha256")"
                [ -n "$expect_tree" ] || fail "incompatible_save" "manifest names no world bytes"
                actual_tree="$(tree_hash "$world" "$LOCK_BASENAME")" || fail "incompatible_save" "world does not hash"
                [ "$actual_tree" = "$expect_tree" ] || fail "incompatible_save" "world bytes diverge from the manifest"
                ;;
            restore-backup)
                RESTORE_RETIRED=""
                restore_backup_world "$manifest" "$world" "$backup" "$nonce"
                retired="$RESTORE_RETIRED"
                ;;
        esac
    fi
    start_previous_owner "$manifest" "$world" "$run_canon" "$previous"
    if [ -n "$retired" ] && [ -d "$retired" ]; then
        rm -rf "$retired"
        manifest_set_field "$manifest" "restore_stage" 'null'
    fi
    echo "OK rollback manifest=$manifest phase=PreviousRunning policy=$policy"
}

cmd_self_test() {
    need_python
    [ -n "${MORNLEA_RUST_SERVER_BIN:-}" ] || fail "activation_failed" "MORNLEA_RUST_SERVER_BIN names the rebuilt Rust binary"
    [ -n "${MORNLEA_PREVIOUS_SERVER_BIN:-}" ] || fail "activation_failed" "MORNLEA_PREVIOUS_SERVER_BIN names the previous Go binary"
    [ -f "$MORNLEA_RUST_SERVER_BIN" ] || fail "activation_failed" "rust binary is absent"
    [ -f "$MORNLEA_PREVIOUS_SERVER_BIN" ] || fail "activation_failed" "previous binary is absent"
    local root
    root="$(py "${TMPDIR:-/tmp}" <<'EOF'
import sys, tempfile
print(tempfile.mkdtemp(prefix="os-", dir=sys.argv[1]))
EOF
)"
    local run="$root/run"
    local world="$run/world"
    local backup="$run/backup"
    mkdir -p "$world" "$run"
    local spawned=""
    cleanup_self_test() {
        for pid in $spawned; do
            kill -9 "$pid" 2>/dev/null || true
        done
        rm -rf "$root"
    }
    trap cleanup_self_test EXIT
    local previous_sha rust_hash
    previous_sha="$(sha256_file "$MORNLEA_PREVIOUS_SERVER_BIN")"
    rust_hash="$(sha256_file "$MORNLEA_RUST_SERVER_BIN")"

    "$MORNLEA_PREVIOUS_SERVER_BIN" --world "$world" --listen "127.0.0.1:0" >>"$root/prepare.log" 2>&1 &
    local prep_pid=$!
    spawned="$spawned $prep_pid"
    local seed_before="" listen_before=""
    local deadline
    deadline=$((SECONDS + GO_READY_TIMEOUT_SECS))
    while [ $SECONDS -lt $deadline ]; do
        kill -0 "$prep_pid" 2>/dev/null || fail "activation_failed" "previous binary exited during prepare"
        listen_before="$(previous_listen_from_log "$root/prepare.log")"
        if [ -n "$listen_before" ] && seed_before="$(login_seed "${listen_before%%:*}" "${listen_before##*:}" "selftest-prepare" 2>/dev/null)"; then
            [ -n "$seed_before" ] && break
            seed_before=""
        fi
        sleep 1
    done
    [ -n "$seed_before" ] || fail "activation_failed" "prepare login never succeeded"
    kill -9 "$prep_pid" 2>/dev/null || true
    wait "$prep_pid" 2>/dev/null || true
    spawned=""
    local world_before
    world_before="$(tree_hash "$world" "$LOCK_BASENAME")" || fail "activation_failed" "prepared world does not hash"
    echo "SELFTEST prepared world seed=$seed_before tree=$world_before"

    bash "$SCRIPT_PATH" activate --world "$world" --backup "$backup" --run-dir "$run" --rust-bin "$MORNLEA_RUST_SERVER_BIN" --previous-bin "$MORNLEA_PREVIOUS_SERVER_BIN" --previous-sha256 "$previous_sha" || fail "activation_failed" "self-test activate refused"
    local manifest="$run/$MANIFEST_BASENAME"
    [ "$(manifest_get "$manifest" "phase")" = "RustRunning" ] || fail "activation_failed" "self-test never reached RustRunning"
    local nonce
    nonce="$(manifest_get "$manifest" "start_nonce")"
    local status
    status="$(control_rpc "$run/$CONTROL_BASENAME" '{"op":"status"}')"
    [ "$(py "$status" <<'EOF'
import json, sys
print(json.loads(sys.argv[1]).get("nonce", ""))
EOF
)" = "$nonce" ] || fail "activation_failed" "control nonce diverges from the manifest"
    [ "$(tree_hash "$world" "$LOCK_BASENAME")" = "$world_before" ] || fail "activation_failed" "rust tenure wrote world bytes"
    echo "SELFTEST activate holds the lock with the manifest nonce"

    bash "$SCRIPT_PATH" rollback --manifest "$manifest" --data-policy compatible || fail "activation_failed" "self-test compatible rollback refused"
    [ "$(manifest_get "$manifest" "phase")" = "PreviousRunning" ] || fail "activation_failed" "self-test never reached PreviousRunning"
    local listen seed_after
    listen="$(cat "$run/previous-listen.addr")"
    seed_after="$(login_seed "${listen%%:*}" "${listen##*:}" "selftest-verify")" || fail "previous_start_failed" "self-test verify login refused"
    [ "$seed_after" = "$seed_before" ] || fail "incompatible_save" "authoritative seed diverged across rollback"
    [ "$(lock_probe "$world")" = "LOCKED" ] || fail "previous_start_failed" "previous runtime never took sole lock"
    echo "SELFTEST compatible rollback preserves seed=$seed_after under sole lock"
    local previous_pid
    previous_pid="$(manifest_get "$manifest" "pid")"
    kill -9 "$previous_pid" 2>/dev/null || true
    wait "$previous_pid" 2>/dev/null || true

    local meta="$world/world.meta"
    py "$meta" <<'EOF'
data = bytearray(open(__import__("sys").argv[1], "rb").read())
data[len(data) // 2] ^= 0xff
open(__import__("sys").argv[1], "wb").write(bytes(data))
EOF
    bash "$SCRIPT_PATH" rollback --manifest "$manifest" --data-policy restore-backup || fail "activation_failed" "self-test restore rollback refused"
    listen="$(cat "$run/previous-listen.addr")"
    seed_after="$(login_seed "${listen%%:*}" "${listen##*:}" "selftest-restore")" || fail "previous_start_failed" "self-test restore login refused"
    [ "$seed_after" = "$seed_before" ] || fail "incompatible_save" "restored seed diverges"
    echo "SELFTEST restore rollback reinstalls seed=$seed_after"
    previous_pid="$(manifest_get "$manifest" "pid")"
    kill -9 "$previous_pid" 2>/dev/null || true
    wait "$previous_pid" 2>/dev/null || true

    local dry_run="$root/dryrun"
    mkdir -p "$dry_run/run"
    cp -R "$world" "$dry_run/run/world"
    local dry_output
    dry_output="$(bash "$SCRIPT_PATH" activate --world "$dry_run/run/world" --backup "$dry_run/run/backup" --run-dir "$dry_run/run" --rust-bin "$MORNLEA_RUST_SERVER_BIN" --previous-bin "$MORNLEA_PREVIOUS_SERVER_BIN" --previous-sha256 "$previous_sha" --dry-run)" || fail "activation_failed" "self-test dry-run refused"
    case "$dry_output" in
        *"$rust_hash"*) ;;
        *) fail "activation_failed" "dry-run never reports the selected rust hash" ;;
    esac
    [ ! -e "$dry_run/run/$MANIFEST_BASENAME" ] || fail "activation_failed" "dry-run writes a manifest"
    [ ! -e "$dry_run/run/backup" ] || fail "activation_failed" "dry-run creates a backup"
    echo "SELFTEST dry-run inspects without qualifying"

    local repo
    repo="$(canon "$(dirname "$SCRIPT_PATH")/..")"
    local dirty
    dirty="$(git -C "$repo" status --porcelain -- Makefile packages/server/cmd/mornlea-server packages/client 2>/dev/null)" || fail "activation_failed" "git status of default startup refused"
    [ -z "$dirty" ] || fail "activation_failed" "default startup paths changed: $dirty"
    local client_refs
    client_refs="$(grep "mornlea-client\|godot" "$SCRIPT_PATH" | grep -v "client_refs" | wc -l | tr -d ' ')"
    [ "$client_refs" = "0" ] || fail "activation_failed" "script references a graphical client"
    echo "SELFTEST default startup unchanged and no foreground client referenced"
    echo "SELFTEST all disposable workflows passed"
    trap - EXIT
    cleanup_self_test
}

case "${1:-}" in
    activate) shift; cmd_activate "$@" ;;
    rollback) shift; cmd_rollback "$@" ;;
    --self-test) shift; [ $# -eq 0 ] || usage; cmd_self_test ;;
    -h|--help|"") usage ;;
    *) usage ;;
esac
