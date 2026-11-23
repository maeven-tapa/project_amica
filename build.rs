use std::{env, path::PathBuf};

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_edition((1, 19));
    slint_build::compile_with_config("src/app/amica.slint", config)
        .expect("compile the Amica interface");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let resource = "src/brand/amica.res";
        let manifest =
            PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("locate the manifest"));
        println!("cargo::rerun-if-changed={resource}");
        println!(
            "cargo::rustc-link-arg-bin=amica={}",
            manifest.join(resource).display()
        );
    }
}
