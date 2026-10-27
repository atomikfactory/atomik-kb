//! Build script: lets `tauri-build` prepare the context (permissions, resources) and, on
//! Windows, gives the test executables the application manifest they would otherwise lack.

// Cargo reads build-script directives from stdout.
#![allow(clippy::print_stdout)]

use std::error::Error;
use std::path::PathBuf;

/// The manifest `tauri-build` embeds in the app binary: a dependency on Common Controls 6.
const COMMON_CONTROLS_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

/// The built-in Google client (ADR-0059 decision 6) is read from the environment by `option_env!` when the shell is
/// compiled (`drive_host`): a change of either variable must recompile it.
const GOOGLE_CLIENT_VARS: [&str; 2] = ["KB_GOOGLE_CLIENT_ID", "KB_GOOGLE_CLIENT_SECRET"];

fn main() -> Result<(), Box<dyn Error>> {
    for variable in GOOGLE_CLIENT_VARS {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    tauri_build::build();
    embed_manifest_in_tests()
}

/// `tauri-build` links the manifest into the app binary only. A test executable that builds
/// a Tauri app on the mock runtime pulls in menu code that imports functions only Common
/// Controls 6 exports, so without the manifest Windows refuses to start it
/// (`STATUS_ENTRYPOINT_NOT_FOUND`). Give the tests the same manifest.
fn embed_manifest_in_tests() -> Result<(), Box<dyn Error>> {
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !windows_msvc {
        return Ok(());
    }
    let manifest = PathBuf::from(std::env::var("OUT_DIR")?).join("tests.manifest");
    std::fs::write(&manifest, COMMON_CONTROLS_MANIFEST)?;
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
    Ok(())
}
