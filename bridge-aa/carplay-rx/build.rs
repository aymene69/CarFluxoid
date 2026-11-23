fn main() {
    println!("cargo:rerun-if-env-changed=CARFLUXOID_VERSION");
    println!("cargo:rerun-if-env-changed=CARFLUXOID_BUILD");
}
