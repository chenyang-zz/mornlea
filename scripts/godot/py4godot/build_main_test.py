import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("build-main.py")
SPEC = importlib.util.spec_from_file_location("build_main", SCRIPT)
if SCRIPT.exists():
    builder = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(builder)
else:
    builder = None

PYTHON_ROOT = (
    Path(__file__).resolve().parents[3]
    / "apps/mornlea-godot/addons/py4godot/cpython-3.14.4-darwin64/python"
)


class MainBuildTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(builder, "Native Py4Godot main builder is missing")

    def test_unsupported_host_refuses_before_output_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "untouched" / "main.dylib"
            with (
                patch.object(builder.platform, "system", return_value="Linux"),
                self.assertRaisesRegex(ValueError, "Darwin-arm64"),
            ):
                builder.build(Path(directory), PYTHON_ROOT, output, 4)
            self.assertFalse(output.parent.exists())

    def test_unpinned_compiler_and_sdk_are_refused(self):
        for clang, sdk in (
            ("Apple clang version 20.0.0", "26.5"),
            ("Apple clang version 21.0.0", "26.4"),
        ):
            with (
                self.subTest(clang=clang, sdk=sdk),
                patch.object(builder.platform, "system", return_value="Darwin"),
                patch.object(builder.platform, "machine", return_value="arm64"),
                patch.object(
                    builder.subprocess, "check_output", side_effect=[clang, sdk]
                ),
                self.assertRaises(ValueError),
            ):
                builder.check_host()

    def test_child_failure_retains_log_and_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "failure.log"
            with self.assertRaises(subprocess.CalledProcessError) as result:
                builder.run(
                    ["/bin/sh", "-c", "echo compiler-failure; exit 19"],
                    Path(directory),
                    log,
                )
            self.assertEqual(result.exception.returncode, 19)
            self.assertIn("compiler-failure", log.read_text())

    def test_native_sources_are_sorted_and_cannot_escape_source_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "meson_scripts").mkdir()
            script = root / "meson_scripts/glob_tools_cpp.py"
            script.write_text("print('b.cpp\\na.cpp')\n")
            (root / "a.cpp").write_text("int a;\n")
            (root / "b.cpp").write_text("int b;\n")
            self.assertEqual(
                builder.main_sources(root, Path("/usr/bin/python3")),
                [root / "a.cpp", root / "b.cpp"],
            )
            script.write_text("print('../escape.cpp')\n")
            with self.assertRaises(ValueError):
                builder.main_sources(root, Path("/usr/bin/python3"))

    def test_generator_failure_stops_before_native_compile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            generators = source / "generation_files"
            generators.mkdir(parents=True)
            (generators / "generate_classes_cpp.py").write_text("pass\n")
            (generators / "generate_class_ids.py").write_text(
                "import sys; assert 'site' not in sys.modules; assert sys.dont_write_bytecode; print('generation refused'); exit(23)\n"
            )
            python_root = root / "python"
            interpreter = python_root / "bin/python3.14"
            interpreter.parent.mkdir(parents=True)
            interpreter.write_text(
                '#!/bin/sh\nif [ "$1" = "-B" ] && [ "$2" = "-I" ]; then echo "3.14.4 arm64"; else exec /usr/bin/python3 "$@"; fi\n'
            )
            interpreter.chmod(0o755)
            for name in ("include/python3.14/Python.h", "lib/libpython3.14.dylib"):
                path = python_root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            (source / "py4godot").mkdir()
            output = root / "output/main.dylib"
            output.parent.mkdir()
            output.write_text("previous artifact")
            with (
                patch.object(builder, "check_host"),
                self.assertRaises(subprocess.CalledProcessError) as result,
            ):
                builder.build(source, python_root, output, 4)
            self.assertEqual(result.exception.returncode, 23)
            self.assertEqual(output.read_text(), "previous artifact")
            logs = output.with_name(output.name + ".build")
            self.assertIn("generation refused", (logs / "generate-00.log").read_text())
            self.assertFalse((logs / "link.log").exists())

    def test_native_compile_failure_keeps_previous_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "broken.cpp"
            source.write_text("this is not C++;\n")
            output = root / "main.dylib"
            output.write_text("previous artifact")
            logs = root / "logs"
            logs.mkdir()
            with self.assertRaises(subprocess.CalledProcessError):
                builder.compile_main(root, root / "python", output, logs, [source], 1)
            self.assertEqual(output.read_text(), "previous artifact")
            self.assertIn("error:", (logs / "compile-0000.log").read_text())
            self.assertFalse((logs / "link.log").exists())

    def test_nil_patch_calls_real_nil_interface_for_both_storage_routes(self):
        patch_file = SCRIPT.parent / "patches/0003-repair-native-nil-construction.patch"
        self.assertTrue(patch_file.exists(), "Native NIL repair patch is missing")
        source = r"""
#include <cassert>
struct PyObject {};
using GDExtensionVariantPtr = void*;
enum GDExtensionVariantType { GDEXTENSION_VARIANT_TYPE_INT };
static void* observed = nullptr;
static void native_nil(void* destination) { observed = destination; }
namespace functions {
auto get_variant_new_nil() { return &native_nil; }
auto get_get_variant_from_type_constructor() {
    return +[](GDExtensionVariantType) { return +[](void*, void*) {}; };
}
}
struct Variant {
    GDExtensionVariantPtr native_ptr;
    void construct_Nil(PyObject* object);
    void construct_Nil_native_ptr(PyObject* object);
};
void Variant::construct_Nil(PyObject* object){
        long long val = 0;
        auto constructor = functions::get_get_variant_from_type_constructor()(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT);
        constructor(native_ptr, &val);
}
void Variant::construct_Nil_native_ptr(PyObject* object){
        int val = 0;
        auto constructor = functions::get_get_variant_from_type_constructor()(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT);
        constructor(&native_ptr, &val);
}
int main() {
    int storage = 0;
    Variant value{&storage};
    value.construct_Nil(nullptr);
    assert(observed == &storage);
    observed = nullptr;
    value.construct_Nil_native_ptr(nullptr);
    assert(observed == &value.native_ptr);
}
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "py4godot/cppcore/Variant.cpp"
            path.parent.mkdir(parents=True)
            path.write_text(source)
            subprocess.run(
                [
                    "patch",
                    "-p1",
                    "--batch",
                    "--forward",
                    "-i",
                    str(patch_file.resolve()),
                ],
                cwd=root,
                check=True,
                capture_output=True,
            )
            executable = root / "nil-check"
            subprocess.run(
                ["clang++", "-std=c++17", str(path), "-o", str(executable)],
                check=True,
                capture_output=True,
            )
            subprocess.run([str(executable)], check=True)


if __name__ == "__main__":
    unittest.main()
