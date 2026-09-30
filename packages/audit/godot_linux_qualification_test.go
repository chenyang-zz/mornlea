package archcheck_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestGodotLinuxQualificationDeniesNetworkSockets(t *testing.T) {
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
		t.Skip("Linux qualification seccomp filter targets x86_64")
	}
	compiler, err := exec.LookPath("cc")
	if err != nil {
		t.Fatal("Linux qualification requires a C compiler")
	}
	fixture := t.TempDir()
	wrapper := filepath.Join(fixture, "deny-network")
	command := exec.Command(compiler, "-std=c11", "-Wall", "-Wextra", "-Werror", "-O2", "-o", wrapper,
		filepath.Join(repositoryRoot(t), "scripts/godot/linux-network-deny.c"))
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("build network denial helper: %v\n%s", err, output)
	}
	probeSource := filepath.Join(fixture, "probe.c")
	writeFile(t, probeSource, []byte(`#include <errno.h>
#include <sys/socket.h>
#include <unistd.h>
int main(void) {
    int families[] = {AF_INET, AF_INET6, AF_PACKET};
    for (unsigned int i = 0; i < sizeof(families) / sizeof(families[0]); i++) {
        errno = 0;
        int descriptor = socket(families[i], SOCK_STREAM, 0);
        if (descriptor != -1 || errno != EPERM) return 1;
    }
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, pair) != 0) return 2;
    close(pair[0]);
    close(pair[1]);
    return 0;
}
`))
	probe := filepath.Join(fixture, "probe")
	if output, err := exec.Command(compiler, "-o", probe, probeSource).CombinedOutput(); err != nil {
		t.Fatalf("build socket probe: %v\n%s", err, output)
	}
	if output, err := exec.Command(wrapper, probe).CombinedOutput(); err != nil {
		t.Fatalf("network sockets must be denied while local sockets work: %v\n%s", err, output)
	}
	command = exec.Command(filepath.Join(repositoryRoot(t), "scripts/godot/deny-network.sh"), probe)
	command.Env = append(os.Environ(), "MORNLEA_PY4GODOT_CACHE_DIR="+filepath.Join(fixture, "helper-cache"))
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("public network denial entry point must compile and execute its helper: %v\n%s", err, output)
	}
	if err := exec.Command(probe).Run(); err == nil {
		t.Fatal("socket probe unexpectedly succeeds without the qualification filter")
	}
	if err := exec.Command(wrapper, "/bin/sh", "-c", "exit 7").Run(); err == nil || err.(*exec.ExitError).ExitCode() != 7 {
		t.Fatalf("qualification wrapper must preserve the probe exit status: %v", err)
	}
}

func TestGodotLinuxQualificationPreservesRuntimeIsolation(t *testing.T) {
	root := repositoryRoot(t)
	check := readBaselineDoc(t, root, "scripts/godot/python-runtime-check.sh")
	for _, required := range []string{"x86_64-unknown-linux-gnu", "linux-network-deny.c", "linux-build-inputs.env", "python-runtime-linux-export-presets.cfg", "MORNLEA_EXPECTED_PYTHON_ROOT", "linux_release.x86_64"} {
		if !strings.Contains(check, required) {
			t.Errorf("Linux runtime qualification is missing %q", required)
		}
	}
	probe := readBaselineDoc(t, root, "apps/mornlea-godot/tests/scripts/python_runtime_probe.py")
	for _, required := range []string{"/proc/self/status", "NoNewPrivs", "Seccomp", "MORNLEA_EXPECTED_PYTHON_ROOT", "sys.flags.isolated", "mornlea_external_runtime_poison"} {
		if !strings.Contains(probe, required) {
			t.Errorf("embedded runtime probe is missing %q", required)
		}
	}
}

func TestGodotLinuxQualificationOnlyClassifiesDeniedEditorListeners(t *testing.T) {
	check := readBaselineDoc(t, repositoryRoot(t), "scripts/godot/python-runtime-check.sh")
	start := strings.Index(check, "filter_editor_network_denials() {")
	if start < 0 {
		t.Fatal("qualification output classifier is missing")
	}
	end := strings.Index(check[start:], "\n}\n")
	if end < 0 {
		t.Fatal("qualification output classifier has no closing brace")
	}
	function := check[start : start+end+3]
	denied := "ERROR: Condition \"_sock == -1\" is true. Returning: FAILED\n" +
		"   at: _inet_open (drivers/unix/net_socket_unix.cpp:288)\n" +
		"ERROR: Condition \"err != OK\" is true. Returning: ERR_CANT_CREATE\n" +
		"   at: listen (core/io/tcp_server.cpp:56)\n"
	for _, test := range []struct{ name, platform, input, want string }{
		{"Linux deliberate denial", "linux64", denied + "ready\n", "ready\n"},
		{"Linux unrelated errors", "linux64", denied + "ERROR: extension failed\n", "ERROR: extension failed\n"},
		{"Linux similar different origin", "linux64", strings.ReplaceAll(denied, "net_socket_unix.cpp:288", "another_file.cpp:288"), strings.ReplaceAll(denied, "net_socket_unix.cpp:288", "another_file.cpp:288")},
		{"macOS preserves all diagnostics", "darwin64", denied, denied},
	} {
		t.Run(test.name, func(t *testing.T) {
			command := exec.Command("bash", "-c", "runtime_platform=\"$1\"\n"+function+"\nfilter_editor_network_denials", "classifier", test.platform)
			command.Stdin = strings.NewReader(test.input)
			output, err := command.CombinedOutput()
			if err != nil || string(output) != test.want {
				t.Fatalf("classifier must preserve every other error: %v\ngot %q\nwant %q", err, output, test.want)
			}
		})
	}
}
