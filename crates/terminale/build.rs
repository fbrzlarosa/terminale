//! Build script: embed the Windows application icon into `terminale.exe`.
//!
//! Runs only when the host is Windows (which is also where the Windows release
//! is built); on macOS/Linux the `winresource` build-dependency isn't pulled in
//! and this whole body compiles away to an empty `main`.

fn main() {
    #[cfg(windows)]
    {
        // Path is relative to this crate's manifest dir (where build scripts run).
        let icon = "../../assets/icons/terminale.ico";
        println!("cargo:rerun-if-changed={icon}");

        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon);
        if let Err(e) = res.compile() {
            // Don't fail the build over a cosmetic icon; surface a warning.
            println!("cargo:warning=could not embed Windows icon ({e})");
        }
    }
}
