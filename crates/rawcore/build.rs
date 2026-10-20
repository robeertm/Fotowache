//! Bindet LibRaw ein und uebersetzt den kleinen C-Anschluss aus `csrc/shim.c`.
//!
//! LibRaw liefert Demosaicing in Profi-Qualitaet (u. a. Markesteijn fuer das
//! 6x6-Farbmuster der RAF-Kameras); eine reine Rust-Alternative gibt es dafuer
//! nicht in vergleichbarer Guete.
//!
//! Zwei Wege:
//! - `LIBRAW_DIR` gesetzt: ein Ordner mit `include/` und `lib/`, in dem LibRaw
//!   STATISCH liegt — gebaut von `scripts/libraw-bauen.sh` (macOS/Linux) oder
//!   von vcpkg (Windows). So entstehen die Pakete fuer die Veroeffentlichung:
//!   ein Programm, das auf fremden Rechnern ohne weitere Bibliotheken laeuft.
//! - sonst pkg-config (Homebrew, apt): LibRaw als gemeinsame Bibliothek.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=csrc/shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LIBRAW_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    let mut build = cc::Build::new();
    build.file("csrc/shim.c");

    if let Ok(dir) = std::env::var("LIBRAW_DIR") {
        statisch(&mut build, &PathBuf::from(dir));
    } else {
        dynamisch(&mut build);
    }

    build.warnings(true).compile("frshim");
}

fn ziel_os() -> String {
    std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default()
}

/// Name der C++-Laufzeit fuer das Zielsystem (LibRaw ist C++).
fn cpp_laufzeit() -> Option<&'static str> {
    match ziel_os().as_str() {
        "macos" | "ios" => Some("c++"),
        "windows" => None, // die Windows-Werkzeugkette bindet die eigene Laufzeit selbst ein
        _ => Some("stdc++"),
    }
}

/// Gibt es `name` als Bibliothek in `lib`? (Unix: libname.a, Windows: name.lib)
fn liegt_da(lib: &Path, name: &str) -> bool {
    lib.join(format!("lib{name}.a")).exists() || lib.join(format!("{name}.lib")).exists()
}

fn statisch(build: &mut cc::Build, dir: &Path) {
    let lib = dir.join("lib");
    build.include(dir.join("include"));
    // Ohne diese Angabe erwartet libraw.h unter Windows eine DLL
    // (__declspec(dllimport)) — das Linken gegen die statische .lib schluege fehl.
    build.define("LIBRAW_NODLL", None);
    println!("cargo:rustc-link-search=native={}", lib.display());

    // Die thread-sichere Ausgabe zuerst: Fotowache dekodiert Miniaturen und
    // Vorschau gleichzeitig in mehreren Faeden.
    let libraw = ["raw_r", "libraw_r", "raw", "libraw"]
        .into_iter()
        .find(|n| liegt_da(&lib, n))
        .unwrap_or_else(|| panic!("LIBRAW_DIR={}: keine LibRaw-Bibliothek in lib/", dir.display()));
    println!("cargo:rustc-link-lib=static={libraw}");

    // Abhaengigkeiten, die LibRaw je nach Bauart mitbringt. Reihenfolge zaehlt
    // beim statischen Linken unter Linux: erst LibRaw, dann was sie braucht.
    // omp/gomp: OpenMP-Laufzeit (macOS: LLVM, Linux: GCC) fuer das
    // mehrkernige Demosaicing.
    for abh in ["jpeg", "lcms2", "jasper", "omp", "gomp"] {
        if liegt_da(&lib, abh) {
            println!("cargo:rustc-link-lib=static={abh}");
        }
    }
    // zlib heisst je nach Version und Bauweise anders: libz.a, zlib.lib oder
    // (neuere CMake-Bauten, z. B. vcpkg) zs.lib / zlibstatic.lib.
    let zlib = ["z", "zlib", "zs", "zlibstatic"].into_iter().find(|n| liegt_da(&lib, n));
    if let Some(z) = zlib {
        println!("cargo:rustc-link-lib=static={z}");
    } else if ziel_os() == "windows" {
        // LibRaw braucht zlib fuer Deflate-DNGs; ohne sie scheitert erst das
        // Linken des Programms mit einem unaufgeloesten `uncompress`.
        let vorhanden: Vec<String> = std::fs::read_dir(&lib)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        panic!("LIBRAW_DIR={}: keine zlib in lib/ (vorhanden: {vorhanden:?})", dir.display());
    }
    if ziel_os() != "windows" && zlib.is_none() {
        // zlib (fuer Deflate-DNGs) gehoert zu jedem macOS und Linux.
        println!("cargo:rustc-link-lib=z");
    }
    if ziel_os() == "linux" && liegt_da(&lib, "gomp") {
        println!("cargo:rustc-link-lib=pthread");
    }
    if let Some(cpp) = cpp_laufzeit() {
        println!("cargo:rustc-link-lib={cpp}");
    }
}

fn dynamisch(build: &mut cc::Build) {
    // Die Verkabelung machen wir selbst (`cargo_metadata(false)`), weil pkg-config auf
    // macOS `-lstdc++` meldet — dort heisst die C++-Laufzeit aber `c++`. Ungefiltert
    // uebernommen bricht das Linken mit „library 'stdc++' not found".
    // Die thread-sichere Ausgabe bevorzugen (mehrere Dekodier-Faeden).
    let probe = ["libraw_r", "libraw"].into_iter().find_map(|name| {
        pkg_config::Config::new()
            .atleast_version("0.20")
            .cargo_metadata(false)
            .probe(name)
            .ok()
    });

    match &probe {
        Some(lib) => {
            for p in &lib.include_paths {
                build.include(p);
            }
            for p in &lib.link_paths {
                println!("cargo:rustc-link-search=native={}", p.display());
            }
            for l in &lib.libs {
                let name = match (l.as_str(), ziel_os().as_str()) {
                    ("stdc++", "macos") => "c++",
                    (other, _) => other,
                };
                println!("cargo:rustc-link-lib={name}");
            }
        }
        None => {
            // Letzter Ausweg: LibRaw ueber die ueblichen Suchpfade des Linkers.
            println!(
                "cargo:warning=libraw ueber pkg-config nicht gefunden — nutze Standardpfade \
                 (oder LIBRAW_DIR auf eine statische LibRaw setzen, siehe scripts/libraw-bauen.sh)"
            );
            println!("cargo:rustc-link-lib=raw");
            if let Some(cpp) = cpp_laufzeit() {
                println!("cargo:rustc-link-lib={cpp}");
            }
        }
    }
}
