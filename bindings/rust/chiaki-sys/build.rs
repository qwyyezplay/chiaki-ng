use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

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
        .define("CHIAKI_ENABLE_STEAMDECK_NATIVE", "OFF")
        .define("CHIAKI_ENABLE_FFMPEG_DECODER", "OFF")
        .define("CHIAKI_ENABLE_STEAM_SHORTCUT", "OFF")
        // Disable optional curl features that require extra system libraries not
        // needed by chiaki's PSN/holepunch usage.
        .define("CURL_USE_LIBPSL", "OFF")
        .define("CURL_USE_LIBIDN2", "OFF")
        .define("USE_NGHTTP2", "OFF");

    if target_os == "windows" && target_env == "gnu" {
        cfg.generator("Ninja");
    }

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
    // nanopb's CMakeLists sets CMAKE_DEBUG_POSTFIX="d" unconditionally on all
    // platforms, so debug builds produce libprotobuf-nanopbd.a everywhere.
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
    // curl's CMakeLists sets CMAKE_DEBUG_POSTFIX="-d" unconditionally on all
    // platforms, so debug builds produce libcurl-d.a everywhere.
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

    // ------------------------------------------------------------------
    // System library search paths
    // ------------------------------------------------------------------
    // Emit platform-specific search paths once here, covering all system
    // libraries below (json-c, miniupnpc, ssh2, openssl, opus).  Centralising
    // them avoids duplicated directives and ensures every pkg_config fallback
    // can locate its files regardless of which probes succeed or fail.
    //
    // Override: set CHIAKI_SYS_LIB_DIRS to a colon-separated (Unix) or
    // semicolon-separated (Windows) list of directories; when set, only those
    // paths are added and the built-in defaults are skipped entirely.
    println!("cargo:rerun-if-env-changed=CHIAKI_SYS_LIB_DIRS");
    if let Ok(extra_dirs) = env::var("CHIAKI_SYS_LIB_DIRS") {
        let sep = if target_os == "windows" { ';' } else { ':' };
        for dir in extra_dirs.split(sep).filter(|d| !d.is_empty()) {
            println!("cargo:rustc-link-search=native={}", dir);
        }
    } else {
        let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        match target_os.as_str() {
            "macos" => {
                // Apple Silicon Homebrew (covers openssl@3, json-c, opus, …)
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
                println!("cargo:rustc-link-search=native=/opt/homebrew/opt/openssl@3/lib");
                // Intel Homebrew
                println!("cargo:rustc-link-search=native=/usr/local/lib");
                println!("cargo:rustc-link-search=native=/usr/local/opt/openssl@3/lib");
            }
            "linux" => {
                println!("cargo:rustc-link-search=native=/usr/local/lib");
                println!("cargo:rustc-link-search=native=/usr/lib");
                // Debian/Ubuntu multi-arch paths — missed by the /usr/lib fallback alone.
                let multiarch = match target_arch.as_str() {
                    "x86_64"  => Some("x86_64-linux-gnu"),
                    "aarch64" => Some("aarch64-linux-gnu"),
                    "arm"     => Some("arm-linux-gnueabihf"),
                    _         => None,
                };
                if let Some(triple) = multiarch {
                    println!("cargo:rustc-link-search=native=/usr/lib/{}", triple);
                    println!("cargo:rustc-link-search=native=/usr/local/lib/{}", triple);
                }
            }
            "windows" if target_env == "gnu" => {
                println!("cargo:rustc-link-search=native=/mingw64/lib");
            }
            _ => {}
        }
    }

    // json-c and miniupnpc — system libraries used by holepunch.c.
    // On macOS CoreServices provides _Gestalt used in takion.c.
    let json_via_pkg_config = pkg_config::probe_library("json-c").is_ok();
    if !json_via_pkg_config {
        println!("cargo:rustc-link-lib=dylib=json-c");
    }

    let miniupnpc_via_pkg_config = pkg_config::probe_library("miniupnpc").is_ok();
    if !miniupnpc_via_pkg_config {
        println!("cargo:rustc-link-lib=dylib=miniupnpc");
    }

    if target_os == "macos" {
        // _Gestalt is in CoreServices (used by takion.c for macOS version detection).
        println!("cargo:rustc-link-lib=framework=CoreServices");
        // _SCDynamicStoreCopyProxies is in SystemConfiguration (used by curl's macos.c).
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
    }

    // zlib — used by curl's content_encoding.c for gzip/deflate decompression.
    // Available as a system library on macOS and most Linux distributions.
    println!("cargo:rustc-link-lib=dylib=z");

    // libssh2 — curl was built with SSH support; link the system library.
    let ssh2_via_pkg_config = pkg_config::probe_library("libssh2").is_ok();
    if !ssh2_via_pkg_config {
        println!("cargo:rustc-link-lib=dylib=ssh2");
    }

    // OpenSSL — both libssl (TLS handshake) and libcrypto (RNG, digest, cipher).
    // curl's bundled OpenSSL backend requires both libssl and libcrypto.
    //
    // Search order:
    //   1. pkg-config (covers standard and Homebrew installs)
    //   2. OPENSSL_LIB_DIR env var — explicit path to the directory with libssl/libcrypto
    //   3. OPENSSL_DIR env var    — root of an OpenSSL install; <OPENSSL_DIR>/lib is used
    //   4. Fall back to the platform search paths emitted by the block above
    println!("cargo:rerun-if-env-changed=OPENSSL_LIB_DIR");
    println!("cargo:rerun-if-env-changed=OPENSSL_DIR");
    let openssl_via_pkg_config = pkg_config::probe_library("openssl").is_ok();
    if !openssl_via_pkg_config {
        if let Ok(lib_dir) = env::var("OPENSSL_LIB_DIR") {
            println!("cargo:rustc-link-search=native={}", lib_dir);
        } else if let Ok(dir) = env::var("OPENSSL_DIR") {
            println!("cargo:rustc-link-search=native={}/lib", dir);
        }
        // If neither env var is set, rely on the platform paths already emitted
        // by the CHIAKI_SYS_LIB_DIRS block above (e.g. /opt/homebrew/opt/openssl@3/lib).
        println!("cargo:rustc-link-lib=dylib=ssl");
        println!("cargo:rustc-link-lib=dylib=crypto");
    }

    // opus — used by chiaki's audio encoder (opusencoder.c).
    let opus_via_pkg_config = pkg_config::probe_library("opus").is_ok();
    if !opus_via_pkg_config {
        println!("cargo:rustc-link-lib=dylib=opus");
    }

    // Windows system libraries required by curl (Schannel TLS, BCrypt RNG)
    // and chiaki's holepunch (IP adapter enumeration).
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=crypt32");   // Cert*/CryptQueryObject/CryptDecodeObjectEx
        println!("cargo:rustc-link-lib=advapi32");   // CryptAcquireContextA/CryptCreateHash etc.
        println!("cargo:rustc-link-lib=bcrypt");     // BCryptGenRandom
        println!("cargo:rustc-link-lib=iphlpapi");   // GetAdaptersInfo
    }

    // Generate Rust bindings using bindgen
    let chiaki_include_dir = repo_root.join("lib/include");

    let mut extra_includes: Vec<PathBuf> = Vec::new();

    // Platform-specific includes
    match target_os.as_str() {
        "windows" if target_env == "gnu" => {
            // MSYS2/MinGW-w64: packages install headers to /mingw64/include
            extra_includes.push(PathBuf::from("/mingw64/include"));
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
            if file_name == "ffmpegdecoder.h" {
                // Excluded: CHIAKI_ENABLE_FFMPEG_DECODER is OFF; FFmpeg headers
                // are not required for the Rust bindings build.
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
