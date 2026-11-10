//! Links the application icon into the Windows executable.
//!
//! Icons are not something a crate can embed for its dependents: Windows reads
//! them from the final image, so the binary crate compiles its own resource
//! script. gpui already links a manifest; an ICON entry is a different resource
//! type, so the two coexist.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=pido.rc");
        println!("cargo:rerun-if-changed=assets/pido.ico");
        // A failed rc run must fail the build, not ship an iconless binary.
        embed_resource::compile("pido.rc", embed_resource::NONE)
            .manifest_required()
            .expect("the icon resource must link");
    }
}
