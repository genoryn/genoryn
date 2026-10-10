// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::{env, fs, io, path::Path};

use cmake::Config;

// MSVC's upstream generator writes configuration.asm into its source directory.
// Keep generated artifacts under Cargo's output directory, preserving the pinned
// repository bytes for review, reproducibility and packaging validation.
fn copy_native_source(source: &Path, destination: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(source)?.file_type().is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "native source root must be a regular directory"));
    }
    fs::create_dir_all(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_native_source(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "native source must contain only regular files and directories"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn main() {
    println!("cargo:rerun-if-changed=RandomX");
    println!("cargo:rerun-if-env-changed=RANDOMX_DIR");
    assert!(env::var_os("RANDOMX_DIR").is_none(), "GENORYN requires the pinned vendored RandomX source; remove RANDOMX_DIR");
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let output = env::var("OUT_DIR").expect("Cargo output directory");
    let native_source = Path::new(&output).join("genoryn-randomx-source");
    if native_source.exists() {
        fs::remove_dir_all(&native_source).expect("remove previous generated native build copy");
    }
    copy_native_source(&Path::new(&manifest).join("RandomX"), &native_source)
        .expect("copy pinned native source into build output");
    let randomx_path = Config::new(&native_source)
        .out_dir(Path::new(&output).join("genoryn-isolated-build"))
        .define("DARCH", "native")
        .build();

    println!("cargo:rustc-link-search=native={}/lib64", randomx_path.display());
    println!("cargo:rustc-link-search=native={}/lib", randomx_path.display());
    println!("cargo:rustc-link-lib=static=randomx");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or("linux".to_string());
    let dylib_name = match target_os.as_str() {
        "freebsd" | "macos" | "ios" => "c++", // FreeBSD, macOS and iOS use "c++"
        "windows" => "msvcrt",                // Use MSVC runtime on Windows
        _ => "stdc++",                        // Default for other systems (Linux, etc.)
    };
    println!("cargo:rustc-link-lib=dylib={}", dylib_name);

    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
