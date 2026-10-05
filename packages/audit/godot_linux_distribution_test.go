package archcheck_test

import (
	"archive/zip"
	"bytes"
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
)

func TestGodotLinuxExportRetainsBootstrapPreloads(t *testing.T) {
	root := repositoryRoot(t)
	presets := readBaselineDoc(t, root, "apps/mornlea-godot/export_presets.cfg")
	_, linuxPreset, ok := strings.Cut(presets, `name="Mornlea Linux x86_64"`)
	if !ok {
		t.Fatal("Linux export preset is missing")
	}
	linuxPreset, _, _ = strings.Cut(linuxPreset, "[preset.")
	filter := regexp.MustCompile(`(?m)^exclude_filter="([^"]*)"$`).FindStringSubmatch(linuxPreset)
	if len(filter) != 2 {
		t.Fatal("Linux exclusion filter is missing")
	}
	bootstrap := readBaselineDoc(t, root, "apps/mornlea-godot/app/bootstrap/bootstrap.gd")
	preloads := regexp.MustCompile(`preload\("res://([^"]+)"\)`).FindAllStringSubmatch(bootstrap, -1)
	if len(preloads) == 0 {
		t.Fatal("Bootstrap preload dependencies were not checked")
	}
	var resources []string
	for _, preload := range preloads {
		resources = append(resources, preload[1])
		if strings.HasSuffix(preload[1], ".tscn") {
			scene := readBaselineDoc(t, root, "apps/mornlea-godot/"+preload[1])
			for _, reference := range regexp.MustCompile(`path="res://([^"]+)"`).FindAllStringSubmatch(scene, -1) {
				resources = append(resources, reference[1])
			}
		}
	}
	for _, resource := range resources {
		for _, pattern := range strings.Split(filter[1], ",") {
			matched, err := filepath.Match(pattern, resource)
			if err != nil {
				t.Fatalf("invalid exclusion pattern %q: %v", pattern, err)
			}
			if matched {
				t.Errorf("Linux export excludes required Bootstrap resource %s", resource)
			}
		}
	}
}

func TestGodotLinuxFetchAndResolver(t *testing.T) {
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
		t.Skip("Linux host resolver")
	}
	fixture := t.TempDir()
	scriptDir := filepath.Join(fixture, "repository/scripts/godot")
	root := repositoryRoot(t)
	for _, name := range []string{"fetch.sh", "godot.sh"} {
		writeExecutable(t, filepath.Join(scriptDir, name), readBaselineDoc(t, root, "scripts/godot/"+name))
	}
	var archive bytes.Buffer
	writer := zip.NewWriter(&archive)
	header := &zip.FileHeader{Name: "Godot_vfixture_linux.x86_64", Method: zip.Store}
	header.SetMode(0o755)
	entry, err := writer.CreateHeader(header)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := entry.Write([]byte("#!/bin/sh\nprintf 'fixture\\n'\n")); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	archivePath := filepath.Join(fixture, "editor.zip")
	templatesPath := filepath.Join(fixture, "templates.tpz")
	writeFile(t, archivePath, archive.Bytes())
	templates := []byte("verified templates fixture")
	writeFile(t, templatesPath, templates)
	pins := fmt.Sprintf("GODOT_VERSION=fixture\nGODOT_LINUX_X86_64_URL=file://%s\nGODOT_LINUX_X86_64_SHA256=%x\nGODOT_EXPORT_TEMPLATES_URL=file://%s\nGODOT_EXPORT_TEMPLATES_SHA256=%x\n", archivePath, sha256.Sum256(archive.Bytes()), templatesPath, sha256.Sum256(templates))
	writeFile(t, filepath.Join(scriptDir, "version.env"), []byte(pins))
	cache := filepath.Join(fixture, "cache")
	command := exec.Command(filepath.Join(scriptDir, "fetch.sh"), "--target", "linux-x86_64", "--cache-dir", cache)
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("Linux fetch: %v\n%s", err, output)
	}
	command = exec.Command(filepath.Join(scriptDir, "godot.sh"), "--print-path")
	command.Env = append(os.Environ(), "MORNLEA_GODOT_CACHE_DIR="+cache, "MORNLEA_GODOT_BIN=")
	output, err := command.CombinedOutput()
	expected := filepath.Join(cache, "fixture/linux-x86_64/Godot_vfixture_linux.x86_64")
	if err != nil || strings.TrimSpace(string(output)) != expected {
		t.Fatalf("Linux resolver: %v\n%s", err, output)
	}
	// A changed cache artifact cannot replace an already verified executable.
	installed := readFile(t, expected)
	writeFile(t, filepath.Join(cache, "fixture/linux-x86_64/Godot_vfixture_linux.x86_64.zip"), []byte("corrupt"))
	command = exec.Command(filepath.Join(scriptDir, "fetch.sh"), "--target", "linux-x86_64", "--cache-dir", cache)
	if output, err := command.CombinedOutput(); err == nil || !strings.Contains(string(output), "checksum mismatch") {
		t.Fatalf("corrupt Linux archive accepted: %v\n%s", err, output)
	}
	if !bytes.Equal(readFile(t, expected), installed) {
		t.Fatal("corrupt archive replaced qualified editor")
	}
}

func TestGodotLinuxExportProtectsUnownedOutput(t *testing.T) {
	output := t.TempDir()
	marker := filepath.Join(output, "user-data")
	writeFile(t, marker, []byte("preserve me"))
	command := exec.Command(filepath.Join(repositoryRoot(t), "scripts/godot/export-linux.sh"), "--output", output)
	got, err := command.CombinedOutput()
	if err == nil || !strings.Contains(string(got), "unowned nonempty output") {
		t.Fatalf("unowned output not rejected: %v\n%s", err, got)
	}
	if string(readFile(t, marker)) != "preserve me" {
		t.Fatal("user file changed")
	}
}
