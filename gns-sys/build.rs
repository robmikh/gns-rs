use std::{path::PathBuf, process::Command};
use std::path::Path;

fn link(lib: impl AsRef<str>) {
    println!("cargo:rustc-link-lib={}", lib.as_ref());
}

fn link_search(build_subpath: impl AsRef<Path>) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out_dir.join(build_subpath).display());
}

// Copied from 'cc'; https://docs.rs/cc/latest/src/cc/lib.rs.html#3073
fn link_stdlib() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap();
    let target_vendor = std::env::var("CARGO_CFG_TARGET_VENDOR").unwrap();

    if &target_os == "windows" && &target_env == "msvc" {
        // No stdlib linking needed for MSVC
    } else if &target_vendor == "apple"
        || &target_os == "freebsd"
        || &target_os == "openbsd"
        || &target_os == "aix"
        || (&target_os == "linux" && &target_env == "ohos")
        || &target_os == "wasi"
    {
        link("c++");
    } else if &target_os == "android" {
        link("c++_shared");
    } else {
        link("stdc++");
    }
}

fn assert_cmd(cmd: &mut Command) {
    let status = cmd.status().unwrap();
    if !status.success() {
        panic!("Failed to exec cmd ({status}): {cmd:?}");
    }
}

fn git_clone(repo_url: &str, dst: &Path, commit: Option<&str>) {
    let exists = if dst.exists() {
        Command::new("git")
            .arg("-C").arg(dst)
            .arg("status")
            .status().unwrap()
            .success()
    } else {
        false
    };
    if !exists {
        // Repo not created yet, clone it
        assert_cmd(Command::new("git")
            .args(["clone", repo_url])
            .arg(dst.as_os_str()));
    }
    if let Some(commit) = commit {
        assert_cmd(Command::new("git")
            .arg("-C").arg(dst)
            .args(["checkout", commit]));
    }
    assert_cmd(Command::new("git")
        .arg("-C").arg(dst)
        .args(["submodule", "update", "--init", "--recursive"]));
}

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let vcpkg_target_triplet = vckpg_target_triplet(&target_os, &target_arch);
    let vcpkg_bootstrap_script = vckpg_bootstrap_script(&target_os);

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    println!("cargo::rerun-if-changed={}", manifest_dir.join("src").display());

    let gns_src_dir = manifest_dir.join("thirdparty").join("GameNetworkingSockets");
    println!("cargo::rerun-if-changed={}", gns_src_dir.join("src").display());
    println!("cargo::rerun-if-changed={}", gns_src_dir.join("include").display());
    println!("cargo::rerun-if-changed={}", gns_src_dir.join("cmake").display());
    println!("cargo::rerun-if-changed={}", gns_src_dir.join("CMakeLists.txt").display());

    let bindings = bindgen::Builder::default()
        .clang_arg(format!("-I{}", gns_src_dir.join("src").join("include").display()))
        .clang_arg(format!("-I{}", gns_src_dir.join("src").join("public").display()))
        .clang_arg(format!("-I{}", gns_src_dir.join("src").join("common").display()))
        .clang_arg("-DSTEAMNETWORKINGSOCKETS_STANDALONELIB")
        .header(gns_src_dir.join("include").join("steam").join("steamnetworkingsockets_flat.h").to_string_lossy())
        .header(gns_src_dir.join("include").join("steam").join("steamnetworkingsockets.h").to_string_lossy())
        .derive_debug(true)
        .derive_default(true)
        .derive_copy(true)
        .derive_partialord(true)
        .derive_ord(true)
        .derive_partialeq(true)
        .derive_eq(true)
        .derive_hash(true)
        .use_core()
        .layout_tests(false)
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: false,
        })
        .clang_arg("-xc++")
        .clang_arg("-std=c++20")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    if std::env::var("DOCS_RS").is_ok() {
        // We're building docs on docs.rs, and don't actually need to compile. Instead, just
        // generate bindings and let docs build from that.
        return
    }

    link_search("build/src");

    link("GameNetworkingSockets_s");

    let gns_src_dir = {
        println!("cargo::rerun-if-changed={}", gns_src_dir.join("vcpkg.json").display());

        // TODO: We can't make changes outside of OUT_DIR, but we need to clone/install vcpkg,
        //  and _only_ on Windows. Upstream GameNetworkingSockets will only find vcpkg if it is
        //  cloned to the root of it's src/ dir; we may want to submit a patch that will let it
        //  find vcpkg elsewhere, to avoid cloning the src/ dir to OUT_DIR.
        let new_dir = out_dir.join("GNS");
        if new_dir.exists() {
            std::fs::remove_dir_all(&new_dir).unwrap();
        }
        dircpy::copy_dir(&gns_src_dir, &new_dir).unwrap();
        new_dir
    };

    let mut c = cmake::Config::new(&gns_src_dir);

    {
        let vcpkg_root = gns_src_dir.join("vcpkg");
        let vcpkg_installed_root = out_dir.join("vcpkg").join("installed");

        println!("cargo::rerun-if-env-changed=GNS_VCPKG_BUILDTREES_ROOT");
        println!("cargo::rerun-if-env-changed=GNS_VCPKG_BUILDTREES_ROOT_NO_CHECK");

        let long_paths_support = long_paths_support();

        let vcpkg_buildtrees_root = match std::env::var("GNS_VCPKG_BUILDTREES_ROOT") {
            Ok(v) => PathBuf::from(v),
            Err(_) => out_dir.join("vcpkg").join("buildtrees"),
        };
        let vcpkg_buildtrees_root_len = vcpkg_buildtrees_root.to_string_lossy().chars().count();
        if (std::env::var("GNS_VCPKG_BUILDTREES_ROOT_NO_CHECK").unwrap_or("".to_owned()) != "true" && !long_paths_support)
            && vcpkg_buildtrees_root_len > 100
        {
            panic!(
                "vcpkg 'buildtrees' root path ('{}') is too long ({} > 100)\
                \n\
                \nvcpkg 'buildtrees' root can use very long paths, which can exceed the\
                \ndefault Windows MAX_PATH of 256, and more importantly the CMake limit of 250.\
                \nA shorter path can be used by setting `GNS_VCPKG_BUILDTREES_ROOT` to a custom\
                \nlocation.\
                \n\
                \nAlternatively, this check can be bypassed by setting\
                \n`GNS_VCPKG_BUILDTREES_ROOT_NO_CHECK=true`, but this will likely result in build\
                \nfailures if you don't know exactly what you are doing.",
                vcpkg_buildtrees_root.display(),
                vcpkg_buildtrees_root_len,
            );
        }

        git_clone(
            "https://github.com/microsoft/vcpkg",
            &vcpkg_root,
            None,
        );
        Command::new(vcpkg_root.join(&vcpkg_bootstrap_script))
            .status()
            .unwrap();
        let buildtrees_root_arg = format!("--x-buildtrees-root={}", vcpkg_buildtrees_root.display());
        assert_cmd(Command::new(vcpkg_root.join("vcpkg"))
            .arg("install")
            .arg(format!("--x-manifest-root={}", gns_src_dir.display()))
            .arg(format!("--triplet={}", vcpkg_target_triplet))
            .arg(format!("--x-install-root={}", vcpkg_installed_root.display()))
            .arg(&buildtrees_root_arg));

        let protobuf = vcpkg_rs_mf::Config::new()
            .vcpkg_root(vcpkg_root.clone())
            .vcpkg_installed_root(vcpkg_installed_root.clone())
            .cargo_metadata(false)
            .copy_dlls(false)
            .target_triplet(&vcpkg_target_triplet)
            .find_package("protobuf")
            .unwrap();

        let openssl = vcpkg_rs_mf::Config::new()
            .vcpkg_root(vcpkg_root.clone())
            .vcpkg_installed_root(vcpkg_installed_root.clone())
            .cargo_metadata(false)
            .copy_dlls(false)
            .target_triplet(&vcpkg_target_triplet)
            .find_package("openssl")
            .unwrap();

        for line in protobuf.cargo_metadata {
            // vcpkg crate doesn't have any method to specify the link metadata as static, so
            // manually do that here
            let line = line.replace(
                "cargo:rustc-link-lib=",
                "cargo:rustc-link-lib=static=",
            );
            println!("{}", line);
        }

        for line in openssl.cargo_metadata {
            // vcpkg crate doesn't have any method to specify the link metadata as static, so
            // manually do that here
            let line = line.replace(
                "cargo:rustc-link-lib=",
                "cargo:rustc-link-lib=static=",
            );
            println!("{}", line);
        }
        
        let profile = std::env::var("PROFILE").unwrap();
        if profile == "release" {
            link_search("build/src/Release");
        } else {
            link_search("build/src/Debug");
        }

        if target_os == "windows" {
            c.define("USE_CRYPTO", "BCrypt");
        }
        c.define("VCPKG_TARGET_TRIPLET", &vcpkg_target_triplet);
        c.define("VCPKG_BUILD_TYPE", profile.clone());
        c.define("VCPKG_INSTALLED_DIR", &vcpkg_installed_root);
        c.define("VCPKG_INSTALL_OPTIONS", &buildtrees_root_arg);
    }
    link_stdlib();

    c.static_crt(false);
    c.define("BUILD_STATIC_LIB", "ON");
    c.define("BUILD_SHARED_LIB", "OFF");
    c.define("OPENSSL_USE_STATIC_LIB", "ON");
    c.define("Protobuf_USE_STATIC_LIBS", "ON");
    c.build();
}

#[cfg(target_os = "windows")]
fn long_paths_support() -> bool {
    use windows_registry::*;

    fn support_impl() -> Result<bool> {
        let key = LOCAL_MACHINE.open("SYSTEM\\CurrentControlSet\\Control\\FileSystem")?;
        let value = key.get_u32("LongPathsEnabled")?;
        Ok(value == 1)
    }

    support_impl().unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn long_paths_support() -> bool {
    true
}

fn vckpg_target_triplet(target_os: &str, target_arch: &str) -> String {
    let vcpkg_arch = match target_arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => panic!("Unknown arch: \"{}\"", target_arch),
    };

    let vcpkg_os = match target_os {
        "macos" => "osx",
        "windows" => "windows-static-md",
        "linux" => "linux",
        _ => panic!("Unknown OS: \"{}\"", target_os),
    };

    format!("{}-{}-release", vcpkg_arch, vcpkg_os)
}

fn vckpg_bootstrap_script(target_os: &str) -> &'static str {
    let script = match target_os {
        "windows" => "bootstrap-vcpkg.bat",
        _ => {
            let is_unix = std::env::var("CARGO_CFG_UNIX").is_ok();
            if !is_unix {
                panic!("Unknown OS: \"{}\"", target_os);
            }
            "bootstrap-vcpkg.sh"
        },
    };

    script
}