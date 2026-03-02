use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // Navigate: bindings/rust/chiaki-sys → bindings/rust → bindings → repo root
    let repo_root = manifest_dir
        .parent()
        .unwrap() // bindings/rust/
        .parent()
        .unwrap() // bindings/
        .parent()
        .unwrap() // repo root
        .to_path_buf();

    // Emit explicit rerun-if-changed directives so Cargo only re-runs this
    // build script when the C library sources actually change.  Without these,
    // any file inside the package directory (or, when CargoCallbacks is used,
    // all transitively-included headers — including generated OUT_DIR files)
    // can trigger an unnecessary full cmake + bindgen rebuild on every
    // `cargo check` invocation.
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("lib").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("CMakeLists.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("third-party/CMakeLists.txt").display()
    );
    // Track the third-party submodules that CMake compiles into static libs.
    for sub in &["jerasure", "gf-complete", "nanopb", "curl"] {
        println!(
            "cargo:rerun-if-changed={}",
            repo_root.join("third-party").join(sub).display()
        );
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Build chiaki-lib using cmake
    let mut cfg = cmake::Config::new(&repo_root);

    cfg.define("CHIAKI_ENABLE_GUI", "OFF")
        .define("CHIAKI_ENABLE_CLI", "OFF")
        .define("CHIAKI_ENABLE_TESTS", "OFF")
        .define("CHIAKI_ENABLE_STEAMDECK_NATIVE", "OFF");

    let profile = env::var("PROFILE").unwrap();

    if profile == "release" {
        cfg.define("CMAKE_BUILD_TYPE", "Release").profile("Release");
    } else {
        cfg.define("CMAKE_BUILD_TYPE", "Debug").profile("Debug");
    }

    let dst = cfg.build_target("chiaki-lib").build();

    // Link chiaki static library
    let build_lib_dir = dst.join("build/lib");
    println!("cargo:rustc-link-search=native={}", build_lib_dir.display());
    println!("cargo:rustc-link-lib=static=chiaki");

    // Link transitive dependencies of libchiaki that CMake handles internally
    // but cargo cannot infer from a static library.
    //
    // jerasure and gf_complete are built as third-party static libs alongside chiaki.
    let build_third_party_dir = dst.join("build/third-party");
    println!(
        "cargo:rustc-link-search=native={}",
        build_third_party_dir.display()
    );
    println!("cargo:rustc-link-lib=static=jerasure");
    println!("cargo:rustc-link-lib=static=gf_complete");

    // nanopb (protobuf) — built by CMake under third-party/nanopb/.
    // CMake appends "d" to the lib name in debug builds.
    let build_nanopb_dir = dst.join("build/third-party/nanopb");
    println!(
        "cargo:rustc-link-search=native={}",
        build_nanopb_dir.display()
    );
    let nanopb_lib = if profile == "release" {
        "protobuf-nanopb"
    } else {
        "protobuf-nanopbd"
    };
    println!("cargo:rustc-link-lib=static={}", nanopb_lib);

    // curl — built by CMake under third-party/curl/lib/ with WebSocket support.
    // CMake appends "d" to the lib name in debug builds.
    let build_curl_dir = dst.join("build/third-party/curl/lib");
    println!(
        "cargo:rustc-link-search=native={}",
        build_curl_dir.display()
    );
    let curl_lib = if profile == "release" {
        "curl"
    } else {
        "curl-d"
    };
    println!("cargo:rustc-link-lib=static={}", curl_lib);

    // json-c and miniupnpc — system libraries used by holepunch.c.
    // On macOS CoreServices provides _Gestalt used in takion.c.
    let json_via_pkg_config = pkg_config::probe_library("json-c").is_ok();
    if !json_via_pkg_config {
        match target_os.as_str() {
            "macos" => {
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
                println!("cargo:rustc-link-search=native=/usr/local/lib");
            }
            "linux" => {
                println!("cargo:rustc-link-search=native=/usr/local/lib");
                println!("cargo:rustc-link-search=native=/usr/lib");
            }
            _ => {}
        }
        println!("cargo:rustc-link-lib=json-c");
    }

    let miniupnpc_via_pkg_config = pkg_config::probe_library("miniupnpc").is_ok();
    if !miniupnpc_via_pkg_config {
        // Search paths already added above for macOS/linux.
        println!("cargo:rustc-link-lib=miniupnpc");
    }

    if target_os == "macos" {
        // _Gestalt is in CoreServices (used by takion.c for macOS version detection).
        println!("cargo:rustc-link-lib=framework=CoreServices");
        // _SCDynamicStoreCopyProxies is in SystemConfiguration (used by curl's macos.c).
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
    }

    // zlib — used by curl's content_encoding.c for gzip/deflate decompression.
    // Available as a system library on macOS and most Linux distributions.
    println!("cargo:rustc-link-lib=z");

    // libssh2 — curl was built with SSH support; link the system library.
    let ssh2_via_pkg_config = pkg_config::probe_library("libssh2").is_ok();
    if !ssh2_via_pkg_config {
        // Search paths for json-c / miniupnpc above already cover /opt/homebrew/lib.
        println!("cargo:rustc-link-lib=ssh2");
    }

    // OpenSSL — both libssl (TLS handshake) and libcrypto (RNG, digest, cipher).
    // curl's bundled OpenSSL backend requires libssl in addition to libcrypto.
    // Probe the combined "openssl" pkg-config package so both are linked together.
    let openssl_via_pkg_config = pkg_config::probe_library("openssl").is_ok();
    if !openssl_via_pkg_config {
        match target_os.as_str() {
            "macos" => {
                // Apple Silicon Homebrew path
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
                println!("cargo:rustc-link-search=native=/opt/homebrew/opt/openssl@3/lib");
                // Intel Homebrew path
                println!("cargo:rustc-link-search=native=/usr/local/lib");
                println!("cargo:rustc-link-search=native=/usr/local/opt/openssl@3/lib");
            }
            "linux" => {
                println!("cargo:rustc-link-search=native=/usr/local/lib");
                println!("cargo:rustc-link-search=native=/usr/lib");
            }
            _ => {}
        }
        println!("cargo:rustc-link-lib=ssl");
        println!("cargo:rustc-link-lib=crypto");
    }

    // Generate Rust bindings using bindgen
    let chiaki_include_dir = repo_root.join("lib/include");

    let mut extra_includes: Vec<PathBuf> = Vec::new();

    // Platform-specific includes
    match target_os.as_str() {
        "windows" => {
            // vcpkg installed include: VCPKG_INSTALLED_DIR/<triplet>/include
            if let Ok(vcpkg_dir) = env::var("VCPKG_INSTALLED_DIR") {
                let triplet =
                    env::var("VCPKG_DEFAULT_TRIPLET").unwrap_or_else(|_| "x64-windows".to_string());
                extra_includes.push(PathBuf::from(&vcpkg_dir).join(&triplet).join("include"));
            }
            // FFmpeg and other pre-built deps extracted to deps/ at repo root
            let deps_include = repo_root.join("deps").join("include");
            if deps_include.exists() {
                extra_includes.push(deps_include);
            }
        }
        "linux" => {
            extra_includes.push(PathBuf::from("/usr/local/include"));
            extra_includes.push(PathBuf::from("/usr/include"));
        }
        "macos" => {
            extra_includes.push(PathBuf::from("/opt/homebrew/include"));
        }
        _ => {}
    }

    let wrapper_h = generate_wrapper_h(&chiaki_include_dir, &out_dir);

    let mut bindings = bindgen::Builder::default()
        .header(wrapper_h.to_str().unwrap())
        .clang_arg(format!("-I{}", chiaki_include_dir.display()));

    for include in &extra_includes {
        bindings = bindings.clang_arg(format!("-I{}", include.display()));
    }

    bindings
        // Disable CargoCallbacks' per-header rerun-if-changed tracking.
        // We already emit explicit directory-level rerun-if-changed directives
        // at the top of this script (lib/, third-party/ submodules, CMakeLists
        // files), which gives us precise control.  Leaving CargoCallbacks'
        // header tracking enabled would additionally track every transitively-
        // included system header (e.g. /usr/include/stddef.h).  If any of those
        // are updated (package manager, OS upgrade) it would spuriously re-run
        // the entire cmake + bindgen pipeline even though chiaki's own sources
        // haven't changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new().rerun_on_header_files(false)))
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}

/// Scan `chiaki_include_dir/chiaki/` for all *.h files (recursively),
/// exclude `pidecoder.h`, and write a wrapper.h into `out_dir`.
/// Returns the path to the generated file.
fn generate_wrapper_h(chiaki_include_dir: &PathBuf, out_dir: &PathBuf) -> PathBuf {
    let chiaki_dir = chiaki_include_dir.join("chiaki");

    let mut headers: Vec<String> = Vec::new();
    collect_headers(&chiaki_dir, &chiaki_dir, &mut headers);
    headers.sort();

    let mut content = String::new();
    for rel in &headers {
        content.push_str(&format!("#include <chiaki/{}>\n", rel));
    }

    let dest = out_dir.join("wrapper.h");
    // Only write if content has changed so we don't update the mtime on every
    // build.  If Cargo (or bindgen's CargoCallbacks) tracks this file via
    // rerun-if-changed, an unconditional write would cause an infinite rebuild
    // loop: build.rs runs → wrapper.h mtime updated → next cargo check sees
    // wrapper.h changed → build.rs runs again.
    let existing = fs::read_to_string(&dest).unwrap_or_default();
    if existing != content {
        fs::write(&dest, content).expect("failed to write generated wrapper.h");
    }
    dest
}

/// Recursively collect `*.h` files under `dir` (relative to `base`) into `out`,
/// skipping `pidecoder.h`. Paths are `/`-separated relative to `base`.
fn collect_headers(base: &PathBuf, dir: &PathBuf, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("failed to read chiaki include dir") {
        let entry = entry.expect("failed to read dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_headers(base, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("h") {
            let file_name = path.file_name().unwrap().to_str().unwrap();
            if file_name == "pidecoder.h" {
                // Excluded: depends on <ilclient.h> (Raspberry Pi VideoCore OpenMAX IL),
                // which is only available on Raspberry Pi hardware and not cross-platform.
                continue;
            }
            // Relative path from the `chiaki/` dir (e.g. "audio.h" or "remote/holepunch.h")
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            out.push(rel);
        }
    }
}
