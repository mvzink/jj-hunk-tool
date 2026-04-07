use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let skill_dir = Path::new("skills/jj-surgeon");
    println!("cargo::rerun-if-changed={}", skill_dir.display());

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("skill_files.rs");

    let mut entries = Vec::new();
    collect_md_files(skill_dir, skill_dir, &mut entries);
    entries.sort();

    let mut code = String::from("const SKILL_FILES: &[(&str, &str)] = &[\n");
    for (rel_path, abs_path) in &entries {
        code.push_str(&format!(
            "    (\"{rel_path}\", include_str!(\"{}\")),\n",
            abs_path
        ));
    }
    code.push_str("];\n");

    fs::write(dest, code).unwrap();
}

fn collect_md_files(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(base, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            let rel = path.strip_prefix(base).unwrap().to_str().unwrap().to_string();
            let abs = fs::canonicalize(&path).unwrap().to_str().unwrap().to_string();
            out.push((rel, abs));
        }
    }
}
