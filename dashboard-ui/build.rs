use slint_build::{CompilerConfiguration, EmbedResourcesKind};

fn main() {
    println!("cargo:rerun-if-changed=ui/dashboard.slint");
    println!("cargo:rerun-if-changed=ui/fonts/Inter-Regular.ttf");
    println!("cargo:rerun-if-changed=ui/fonts/Inter-SemiBold.ttf");
    println!("cargo:rerun-if-changed=ui/fonts/Inter-Bold.ttf");
    println!("cargo:rerun-if-env-changed=CARFLUXOID_VERSION");
    println!("cargo:rerun-if-env-changed=CARFLUXOID_BUILD");

    let embed = if std::env::var_os("CARGO_FEATURE_PREVIEW").is_some() {
        EmbedResourcesKind::EmbedFiles
    } else {
        EmbedResourcesKind::EmbedForSoftwareRenderer
    };

    let config = CompilerConfiguration::new().embed_resources(embed);
    slint_build::compile_with_config("ui/dashboard.slint", config)
        .expect("Failed to compile dashboard.slint");
}
