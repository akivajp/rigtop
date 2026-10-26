//! Embeds the icon in the Windows executable (Explorer, the Start menu and
//! shortcuts show it). Only when building on Windows, where the resource
//! compiler is; elsewhere there is nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=assets/rigtop.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/rigtop.ico");
        resource
            .compile()
            .expect("failed to embed the Windows icon");
    }
}
