#!/usr/bin/env python3
"""Exercise exact vendored pure helpers without a native failure path."""
import os, pathlib, subprocess, tempfile


def main():
    repository = pathlib.Path(__file__).resolve().parents[2]
    source = (repository / 'third-party/randomx-rs/src/lib.rs').read_text()
    start = source.index('fn checked_vm_pointer(')
    end = source.index('\n}\n', start) + 3
    guard = source[start:end]
    build = (repository / 'third-party/randomx-rs/build.rs').read_text()
    start = build.index('fn copy_native_source(')
    end = build.index('\n}\n', start) + 3
    copier = build[start:end]
    harness = '''#![allow(non_camel_case_types)]
use std::{fs, io, path::Path};
#[derive(Debug, PartialEq)]
enum RandomXError { CreationError(String) }
struct randomx_vm { _opaque: u8 }
''' + guard + copier + '''
#[test]
fn null_is_a_creation_error() {
    assert_eq!(checked_vm_pointer(std::ptr::null_mut()),
        Err(RandomXError::CreationError("Failed to allocate VM".into())));
}
#[test]
fn valid_pointer_is_preserved_without_dereference() {
    let mut fixture = randomx_vm { _opaque: 0 };
    let pointer = &mut fixture as *mut randomx_vm;
    assert_eq!(checked_vm_pointer(pointer).unwrap(), pointer);
}
#[test]
fn generated_output_cannot_modify_pinned_source() {
    let root = std::env::var("GENORYN_TEST_FIXTURE").unwrap();
    let root = Path::new(&root);
    let source = root.join("source");
    let output = root.join("output");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/configuration.asm"), b"pinned\\n").unwrap();
    fs::write(source.join("z.c"), b"native input").unwrap();
    copy_native_source(&source, &output).unwrap();
    assert_eq!(fs::read(output.join("nested/configuration.asm")).unwrap(), b"pinned\\n");
    assert_eq!(fs::read(output.join("z.c")).unwrap(), b"native input");
    fs::write(output.join("nested/configuration.asm"), b"generated\\r\\n").unwrap();
    assert_eq!(fs::read(source.join("nested/configuration.asm")).unwrap(), b"pinned\\n");
}
#[cfg(unix)]
#[test]
fn source_symlinks_are_rejected() {
    let root = std::env::var("GENORYN_TEST_FIXTURE").unwrap();
    let root = Path::new(&root);
    let source = root.join("symlink-source");
    fs::create_dir_all(&source).unwrap();
    let outside = root.join("outside");
    fs::write(&outside, b"outside input").unwrap();
    std::os::unix::fs::symlink(&outside, source.join("input.c")).unwrap();
    assert_eq!(copy_native_source(&source, &root.join("symlink-output")).unwrap_err().kind(), io::ErrorKind::InvalidData);
    let directory_link = root.join("symlink-root");
    std::os::unix::fs::symlink(&source, &directory_link).unwrap();
    assert_eq!(copy_native_source(&directory_link, &root.join("root-link-output")).unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert!(!root.join("root-link-output").exists());
}
'''
    with tempfile.TemporaryDirectory(prefix='genoryn-randomx-guard-') as directory:
        root = pathlib.Path(directory)
        file = root / 'guard.rs'
        file.write_text(harness)
        executable = root / ('guard.exe' if os.name == 'nt' else 'guard')
        subprocess.run(['rustc', '--edition=2024', '--test', str(file), '-o', str(executable)], check=True, timeout=60)
        subprocess.run([str(executable)], env=dict(os.environ, GENORYN_TEST_FIXTURE=str(root)), check=True, timeout=30)
        # Compile the actual build entry point with a nonexecuting CMake stub.
        # A rejected override must fail before any source copy or native build.
        stub = '''mod cmake {
    pub struct Config;
    impl Config {
        pub fn new(_: impl AsRef<std::path::Path>) -> Self { panic!("native build must not be reached") }
        pub fn out_dir(&mut self, _: impl AsRef<std::path::Path>) -> &mut Self { self }
        pub fn define(&mut self, _: &str, _: &str) -> &mut Self { self }
        pub fn build(&mut self) -> std::path::PathBuf { unreachable!() }
    }
}
'''
        build_file = root / 'build_guard.rs'
        build_file.write_text(stub + build)
        build_executable = root / ('build_guard.exe' if os.name == 'nt' else 'build_guard')
        subprocess.run(['rustc', '--edition=2024', str(build_file), '-o', str(build_executable)], check=True, timeout=60)
        output = root / 'override-output'
        result = subprocess.run([str(build_executable)], env=dict(os.environ, RANDOMX_DIR=str(root / 'unselected-source'), OUT_DIR=str(output)), capture_output=True, text=True, timeout=30)
        assert result.returncode != 0 and 'requires the pinned vendored RandomX source' in result.stderr
        assert not output.exists()
        print('External source override rejected before file generation or native configuration.')


if __name__ == '__main__':
    main()
