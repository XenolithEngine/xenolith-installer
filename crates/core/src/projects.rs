//! Project management: scaffold a buildable graphical Xenolith project in any
//! directory, and track created projects in a small registry.
//!
//! A project is the engine's window app (`src/` + `resources/` + a generated
//! `Makefile`) that points `STAPPLER_ROOT` at one installed engine version (so
//! the engine version is per-project). The engine's `make/universal.mk` drives
//! the build; the host binary lands in `stappler-build/<host-triple>/<name>`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dirs::Layout;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// The engine version (ref) this project builds against — its STAPPLER_ROOT.
    pub engine: String,
    /// Default build target (triple). Empty in legacy entries → resolved to the
    /// host at build time.
    #[serde(default)]
    pub target: String,
    /// Build tool driving the project's Makefile (`make` or `xlmake`). Empty in
    /// legacy entries → treated as `make` at build time.
    #[serde(default)]
    pub make_tool: String,
    pub created_at: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<Project>,
}

impl ProjectRegistry {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json)
    }

    /// Insert, replacing any existing entry with the same path.
    pub fn add(&mut self, project: Project) {
        self.projects.retain(|p| p.path != project.path);
        self.projects.push(project);
    }

    /// Drop the entry at `path` from the list (does not touch files on disk).
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.projects.len();
        self.projects.retain(|p| p.path != path);
        before != self.projects.len()
    }
}

fn list_subdirs(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Installed engine versions (the subdirs of `engines/`).
pub fn installed_engines(layout: &Layout) -> Vec<String> {
    list_subdirs(&layout.engines_dir())
}

/// Installed target sysroots — the things a project can build *for* (subdirs of
/// the toolchain store's `targets/`).
pub fn installed_targets(layout: &Layout) -> Vec<String> {
    list_subdirs(&layout.toolchains_store_dir().join("targets"))
}

/// The build tool to use when a project records none (legacy/empty): `xlmake` on
/// Windows (the Windows toolchain ships no GNU `make`), `make` everywhere else.
pub fn default_make_tool() -> &'static str {
    if std::env::consts::OS == "windows" {
        "xlmake"
    } else {
        "make"
    }
}

/// Build tools that can drive a project's Makefile, in preference order — the
/// first one present is the selector's default (GNU `make`, then `xlmake`). Both
/// ship in the host toolchain's `bin/`; the Windows toolchain ships only `xlmake`,
/// so there the choice collapses to one (no selector).
pub const MAKE_TOOLS: &[&str] = &["make", "xlmake"];

/// The make-tool binaries present in the host toolchain `bin/`, in [`MAKE_TOOLS`]
/// preference order. A tool may carry a `.exe` suffix on Windows.
pub fn available_make_tools(host_bin: &Path) -> Vec<String> {
    MAKE_TOOLS
        .iter()
        .filter(|t| host_bin.join(t).is_file() || host_bin.join(format!("{t}.exe")).is_file())
        .map(|t| (*t).to_string())
        .collect()
}

/// A make-tool binary name with the host OS's executable suffix (`xlmake` →
/// `xlmake.exe` on Windows).
pub fn make_binary_name(tool: &str) -> String {
    if std::env::consts::OS == "windows" {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

/// A valid project name: non-empty and make/path-safe (letters, digits, `-`,
/// `_` — no spaces). It is used verbatim as both the folder and executable name.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Sanitize a project name into a make-safe executable identifier.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "app".to_string()
    } else {
        s
    }
}

// Vendored project template files (substituted/written by `scaffold`). The C++
// scene is a minimal empty window with a single label; it compiles against the
// engine's headers via STAPPLER_ROOT. Kept in the repo (not generated, not
// copied from the engine demo) so the starting project is intentionally minimal.
const MAKEFILE_TMPL: &str = include_str!("../templates/Makefile.tmpl");
const LAUNCH_TMPL: &str = include_str!("../templates/launch.json");
const SETTINGS_TMPL: &str = include_str!("../templates/settings.json");
const SCENE_H: &str = include_str!("../templates/src/ExampleScene.h");
const SCENE_CPP: &str = include_str!("../templates/src/ExampleScene.cpp");

/// A path in forward-slash form. GNU make REQUIRES `/` (a backslash is an escape /
/// line-continuation, so a Windows `C:\…` `STAPPLER_ROOT` breaks the build), and
/// JSON needs it too (`\` is an invalid escape). Forward slashes work on every OS,
/// including Windows. No-op where the path already has none.
pub fn make_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// Force Unix (LF) line endings. The templates are embedded via `include_str!`,
/// which can pick up CRLF (e.g. an autocrlf checkout on a Windows CI builder), and
/// GNU make breaks on `\r` — a trailing carriage return gets folded into variable
/// values and recipe lines, producing bogus paths. Write LF on every platform.
fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Substitute the `{{…}}` placeholders shared by the project templates.
fn render(
    tmpl: &str,
    engine_root: &Path,
    host_bin: &Path,
    host_triple: &str,
    exe: &str,
    make_bin: &str,
) -> String {
    lf(&tmpl
        .replace("{{STAPPLER_ROOT}}", &make_path(engine_root))
        .replace("{{HOST_BIN}}", &make_path(host_bin))
        .replace("{{HOST_TRIPLE}}", host_triple)
        .replace("{{MAKE_TOOL}}", make_bin)
        .replace("{{EXE}}", exe))
}

/// The subdir the engine nests build output under — named after the host
/// compiler binary: `clang.exe` on Windows, `cc` on macOS/Linux.
pub fn host_cc_subdir() -> &'static str {
    if std::env::consts::OS == "windows" {
        "clang.exe"
    } else {
        "cc"
    }
}

/// Project-relative path of the built binary for the running OS. macOS produces
/// an `.app` bundle; Windows a `.exe`; elsewhere a plain ELF binary.
fn host_binary_rel(host_triple: &str, exe: &str) -> String {
    let base = format!("stappler-build/{host_triple}/debug/{}", host_cc_subdir());
    match std::env::consts::OS {
        "macos" => format!("{base}/{exe}.app/Contents/MacOS/{exe}"),
        "windows" => format!("{base}/{exe}.exe"),
        _ => format!("{base}/{exe}"),
    }
}

/// Create a buildable graphical project at `dir`, wired to `engine_root` (the
/// unpacked engine = STAPPLER_ROOT): writes a minimal labelled scene (`src/`), a
/// `Makefile`, a `.vscode/` config (build via the Makefile + an lldb-dap launch,
/// pointing at the host toolchain `host_bin`) and the engine's `.clang-format`.
/// Does not overwrite an existing `Makefile`.
pub fn scaffold(
    dir: &Path,
    name: &str,
    engine_root: &Path,
    host_triple: &str,
    host_bin: &Path,
    make_tool: &str,
) -> std::io::Result<()> {
    if !engine_root.join("make/universal.mk").is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("engine not found at {}", engine_root.display()),
        ));
    }
    let src = dir.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(src.join("ExampleScene.h"), lf(SCENE_H))?;
    std::fs::write(src.join("ExampleScene.cpp"), lf(SCENE_CPP))?;

    // Carry the engine's formatting config so format-on-save matches upstream.
    let clang_format = engine_root.join(".clang-format");
    if clang_format.is_file() {
        let _ = std::fs::copy(&clang_format, dir.join(".clang-format"));
    }

    let exe = sanitize_name(name);
    // Empty (e.g. a legacy caller) → the historical default of GNU `make`.
    let tool = if make_tool.is_empty() {
        "make"
    } else {
        make_tool
    };
    let make_bin = make_binary_name(tool);
    let makefile = dir.join("Makefile");
    if !makefile.exists() {
        std::fs::write(
            &makefile,
            render(
                MAKEFILE_TMPL,
                engine_root,
                host_bin,
                host_triple,
                &exe,
                &make_bin,
            ),
        )?;
    }

    write_vscode(dir, engine_root, host_triple, host_bin, &exe, &make_bin)
}

/// Write `.vscode/{launch,settings}.json` for the project. These files are fully
/// generated (no user content), so regenerating them is safe — unlike `src/` and
/// the `Makefile`, which `scaffold` never clobbers. `make_bin` is the build-tool
/// binary name that drives `makefile.makePath`.
fn write_vscode(
    dir: &Path,
    engine_root: &Path,
    host_triple: &str,
    host_bin: &Path,
    exe: &str,
    make_bin: &str,
) -> std::io::Result<()> {
    let vscode = dir.join(".vscode");
    std::fs::create_dir_all(&vscode)?;
    let binary = host_binary_rel(host_triple, exe);
    let render_vscode = |tmpl: &str| {
        render(tmpl, engine_root, host_bin, host_triple, exe, make_bin)
            .replace("{{BINARY_PATH}}", &binary)
    };
    std::fs::write(vscode.join("launch.json"), render_vscode(LAUNCH_TMPL))?;
    std::fs::write(vscode.join("settings.json"), render_vscode(SETTINGS_TMPL))?;
    Ok(())
}

/// Switch an existing project's build tool: rewrite its `.vscode` config so VS
/// Code's `makefile.makePath` points at `make_tool`. Leaves `src/` and the
/// `Makefile` untouched (the Makefile is make-syntax, driven by whichever tool).
pub fn set_make_tool(
    dir: &Path,
    name: &str,
    engine_root: &Path,
    host_triple: &str,
    host_bin: &Path,
    make_tool: &str,
) -> std::io::Result<()> {
    let exe = sanitize_name(name);
    let tool = if make_tool.is_empty() {
        "make"
    } else {
        make_tool
    };
    let make_bin = make_binary_name(tool);
    write_vscode(dir, engine_root, host_triple, host_bin, &exe, &make_bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(path: &str) -> Project {
        Project {
            name: "Demo".into(),
            path: PathBuf::from(path),
            engine: "master".into(),
            target: "aarch64-apple-macosx".into(),
            make_tool: "make".into(),
            created_at: "2026-06-10T00:00:00Z".into(),
        }
    }

    #[test]
    fn registry_round_trips_and_dedups_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("projects.json");
        let mut reg = ProjectRegistry::default();
        reg.add(project("/a"));
        reg.add(project("/b"));
        reg.add(project("/a")); // replace, not duplicate
        assert_eq!(reg.projects.len(), 2);
        reg.save(&file).unwrap();

        let mut loaded = ProjectRegistry::load(&file).unwrap();
        assert_eq!(loaded.projects.len(), 2);
        assert!(loaded.remove(Path::new("/a")));
        assert_eq!(loaded.projects.len(), 1);
        assert!(!loaded.remove(Path::new("/a"))); // already gone
    }

    #[test]
    fn load_missing_registry_is_empty() {
        let reg = ProjectRegistry::load(Path::new("/no/such/projects.json")).unwrap();
        assert!(reg.projects.is_empty());
    }

    #[test]
    fn validates_project_names() {
        assert!(is_valid_name("my-app_1"));
        assert!(is_valid_name("Game"));
        assert!(!is_valid_name("")); // empty
        assert!(!is_valid_name("my app")); // space
        assert!(!is_valid_name("app/x")); // separator
    }

    #[test]
    fn sanitize_makes_make_safe_identifiers() {
        assert_eq!(sanitize_name("My Cool App!"), "My_Cool_App_");
        assert_eq!(sanitize_name("  "), "app");
        assert_eq!(sanitize_name("ok-name_1"), "ok-name_1");
    }

    /// A fake engine: just enough for `scaffold` to accept it (the build-system
    /// marker `make/universal.mk` + a `.clang-format` it carries over).
    fn fake_engine(root: &Path) {
        std::fs::create_dir_all(root.join("make")).unwrap();
        std::fs::write(root.join("make/universal.mk"), b"# universal\n").unwrap();
        std::fs::write(root.join(".clang-format"), b"BasedOnStyle: LLVM\n").unwrap();
    }

    const HOST: &str = "aarch64-apple-macosx";

    #[test]
    fn scaffold_writes_minimal_scene_makefile_and_vscode() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        fake_engine(&engine);
        let host_bin = Path::new("/x/toolchains/hosts/aarch64-apple-macosx/bin");
        let proj = dir.path().join("MyGame");
        scaffold(&proj, "My Game", &engine, HOST, host_bin, "xlmake").unwrap();

        let mk = std::fs::read_to_string(proj.join("Makefile")).unwrap();
        // Rendered paths are forward-slashed (make requires it; Windows-safe)…
        assert!(mk.contains(&format!("STAPPLER_ROOT ?= {}", make_path(&engine))));
        // …and line endings are LF (a stray CR breaks GNU make).
        assert!(!mk.contains('\r'));
        assert!(mk.contains("LOCAL_EXECUTABLE := My_Game"));
        assert!(mk.contains("xenolith_application"));
        assert!(mk.contains("include $(STAPPLER_ROOT)/make/universal.mk"));
        // minimal scene (vendored) + .clang-format written
        let scene = std::fs::read_to_string(proj.join("src/ExampleScene.cpp")).unwrap();
        assert!(scene.contains("Rc<Label>::create"));
        assert!(scene.contains("DEFINE_PRIMARY_SCENE_CLASS(ExampleScene)"));
        assert!(proj.join("src/ExampleScene.h").exists());
        assert!(proj.join(".clang-format").exists());

        // .vscode wired to the host toolchain, with placeholders substituted
        let settings = std::fs::read_to_string(proj.join(".vscode/settings.json")).unwrap();
        assert!(settings.contains(&format!("{}/clang-21", make_path(host_bin))));
        assert!(settings.contains(&format!("{}/lldb-dap", make_path(host_bin))));
        // The chosen build tool drives `makefile.makePath` (xlmake here, not make).
        assert!(settings.contains(&format!(
            "\"makefile.makePath\": \"{}/xlmake\"",
            make_path(host_bin)
        )));
        assert!(!settings.contains("/make\""));
        // binaryPath is OS-specific; the common prefix is present on every OS.
        assert!(settings.contains(&format!("stappler-build/{HOST}/debug/cc/My_Game")));
        assert!(!settings.contains("{{"));
        let launch = std::fs::read_to_string(proj.join(".vscode/launch.json")).unwrap();
        assert!(launch.contains("lldb-dap"));
        assert!(launch.contains(&format!("stappler-build/{HOST}/debug/cc/My_Game")));
        assert!(!launch.contains("{{"));
    }

    #[test]
    fn set_make_tool_rewrites_makepath_without_touching_sources() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        fake_engine(&engine);
        let host_bin = Path::new("/x/toolchains/hosts/aarch64-apple-macosx/bin");
        let proj = dir.path().join("MyGame");
        scaffold(&proj, "My Game", &engine, HOST, host_bin, "make").unwrap();

        // Initially makePath uses `make`; user code is the vendored scene.
        let settings0 = std::fs::read_to_string(proj.join(".vscode/settings.json")).unwrap();
        assert!(settings0.contains(&format!(
            "\"makefile.makePath\": \"{}/make\"",
            make_path(host_bin)
        )));
        std::fs::write(proj.join("src/ExampleScene.cpp"), b"// MY EDIT\n").unwrap();

        // Switch to xlmake.
        set_make_tool(&proj, "My Game", &engine, HOST, host_bin, "xlmake").unwrap();
        let settings1 = std::fs::read_to_string(proj.join(".vscode/settings.json")).unwrap();
        assert!(settings1.contains(&format!(
            "\"makefile.makePath\": \"{}/xlmake\"",
            make_path(host_bin)
        )));
        // src/ is user-owned — must NOT be clobbered.
        assert_eq!(
            std::fs::read_to_string(proj.join("src/ExampleScene.cpp")).unwrap(),
            "// MY EDIT\n"
        );
    }

    #[test]
    fn scaffold_without_template_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = scaffold(
            &dir.path().join("p"),
            "x",
            Path::new("/no/engine"),
            HOST,
            Path::new("/b"),
            "make",
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn scaffold_does_not_clobber_existing_makefile() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        fake_engine(&engine);
        let proj = dir.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("Makefile"), b"custom").unwrap();
        scaffold(&proj, "x", &engine, HOST, Path::new("/b"), "make").unwrap();
        assert_eq!(std::fs::read(proj.join("Makefile")).unwrap(), b"custom");
    }

    #[test]
    fn detects_available_make_tools_in_preference_order() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        // Nothing yet.
        assert!(available_make_tools(bin).is_empty());
        // Only xlmake (the Windows case) → no choice, just xlmake.
        std::fs::write(bin.join("xlmake"), b"#!/bin/sh\n").unwrap();
        assert_eq!(available_make_tools(bin), vec!["xlmake"]);
        // Both → make first (the default), then xlmake.
        std::fs::write(bin.join("make"), b"#!/bin/sh\n").unwrap();
        assert_eq!(available_make_tools(bin), vec!["make", "xlmake"]);
    }

    #[test]
    fn make_binary_name_is_plain_off_windows() {
        // Tests run on the host OS; off Windows there is no `.exe` suffix.
        if std::env::consts::OS != "windows" {
            assert_eq!(make_binary_name("xlmake"), "xlmake");
        }
    }

    #[test]
    fn lists_installed_engines_and_targets() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        std::fs::create_dir_all(layout.engine_dir("master")).unwrap();
        std::fs::create_dir_all(layout.engine_dir("v0.1")).unwrap();
        assert_eq!(installed_engines(&layout), vec!["master", "v0.1"]);

        std::fs::create_dir_all(
            layout
                .toolchains_store_dir()
                .join("targets/aarch64-apple-macosx"),
        )
        .unwrap();
        std::fs::create_dir_all(
            layout
                .toolchains_store_dir()
                .join("targets/x86_64-unknown-linux-gnu"),
        )
        .unwrap();
        assert_eq!(
            installed_targets(&layout),
            vec!["aarch64-apple-macosx", "x86_64-unknown-linux-gnu"]
        );
    }
}
