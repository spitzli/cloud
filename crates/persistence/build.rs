fn main() {
    // sqlx::migrate! tracks existing SQL contents; Cargo also needs to notice new files.
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=../identity/migrations");
    println!("cargo:rerun-if-changed=../oauth/migrations");
    println!("cargo:rerun-if-changed=../hosting/migrations");
    println!("cargo:rerun-if-changed=../agent-control/migrations");
    println!("cargo:rerun-if-changed=../access/migrations");
    println!("cargo:rerun-if-changed=../browser-session/migrations");
    println!("cargo:rerun-if-changed=../build/migrations");
    println!("cargo:rerun-if-changed=../databases/migrations");
    println!("cargo:rerun-if-changed=../mail/migrations");
    println!("cargo:rerun-if-changed=../plugin-host/migrations");
    println!("cargo:rerun-if-changed=../../legacy/control-plane/drizzle");
}
