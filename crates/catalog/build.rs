//! Embeds `data/catalog` into the crate.
//!
//! Generates `$OUT_DIR/embedded.rs` with an `include_str!` per category YAML file and per
//! content Markdown file, plus `CATALOG_VERSION`, a hash over every input. Adding a product's
//! content file therefore needs no code change.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let data_dir = manifest_dir.join("../../data/catalog");
    let data_dir = data_dir
        .canonicalize()
        .unwrap_or_else(|e| panic!("catalog data dir {}: {e}", data_dir.display()));
    println!("cargo::rerun-if-changed={}", data_dir.display());

    let yaml = list_files(&data_dir, "yaml");
    let content = list_files(&data_dir.join("content"), "md");

    let mut hash = Fnv::new();
    let mut out = String::new();
    out.push_str("/// Category YAML files: (file stem, contents).\n");
    out.push_str("pub(crate) const CATEGORY_FILES: &[(&str, &str)] = &[\n");
    for path in &yaml {
        emit(&mut out, &mut hash, "yaml", path);
    }
    out.push_str("];\n\n/// Product content files: (product id, contents).\n");
    out.push_str("pub(crate) const CONTENT_FILES: &[(&str, &str)] = &[\n");
    for path in &content {
        emit(&mut out, &mut hash, "content", path);
    }
    out.push_str("];\n\n");
    writeln!(
        out,
        "/// Hash of every embedded catalog input; changes whenever the catalog data changes.\n\
         pub const CATALOG_VERSION: &str = \"{:016x}\";",
        hash.0
    )
    .unwrap();

    let out_path = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("embedded.rs");
    fs::write(&out_path, out).unwrap_or_else(|e| panic!("write {}: {e}", out_path.display()));
}

/// Files with `extension` directly in `dir`, sorted by name. A missing directory is empty.
fn list_files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .map(|e| {
            e.unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == extension))
        .collect();
    files.sort();
    files
}

fn emit(out: &mut String, hash: &mut Fnv, kind: &str, path: &Path) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("non UTF-8 file name: {}", path.display()));
    let contents = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    hash.update(kind.as_bytes());
    hash.update(stem.as_bytes());
    hash.update(&(contents.len() as u64).to_le_bytes());
    hash.update(&contents);
    let path = path
        .to_str()
        .unwrap_or_else(|| panic!("non UTF-8 path: {}", path.display()));
    writeln!(out, "    ({stem:?}, include_str!({path:?})),").unwrap();
}

/// 64-bit FNV-1a (same as `techdepo_domain::fnv1a64`).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
