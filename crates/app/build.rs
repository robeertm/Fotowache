//! Unter Windows das Programmsymbol in die fotowache.exe einbetten — Dateimanager,
//! Taskleiste und Startmenue zeigen sonst ein leeres Standardsymbol. Auf den
//! anderen Systemen gibt es hier nichts zu tun (macOS: Symbol im .app-Paket,
//! Linux: .desktop-Datei).

fn main() {
    println!("cargo:rerun-if-changed=assets/fotowache.rc");
    println!("cargo:rerun-if-changed=assets/fotowache.ico");
    #[cfg(windows)]
    embed_resource::compile("assets/fotowache.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("Windows-Ressource (Symbol) einbinden");
}
